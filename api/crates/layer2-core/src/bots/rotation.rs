use super::*;

impl BotWorker {
    pub(super) async fn rotate(&self, bot: &mut Bot) -> Result<(), BotError> {
        let fresh = bot
            .work
            .as_ref()
            .is_some_and(|work| work.step == "admitted");
        self.step(bot, "read_rotation_channel").await?;
        let current = self
            .external
            .channel(bot)
            .await?
            .ok_or(BotError::Unavailable)?;
        let replacement = bot
            .resources
            .replacement
            .clone()
            .ok_or(BotError::Conflict)?;
        if current.uses(&replacement) {
            // A prior worker committed the replacement but lost its reply or
            // claim. Recover without requiring the lost plaintext credential.
            return self.finish_rotation(bot, current, replacement).await;
        }
        if !current.uses(&bot.resources.credentials) {
            return Err(BotError::Unavailable);
        }
        if !fresh {
            self.effect(bot, "restore_retained_binding").await?;
            let restored = self
                .external
                .configure_channel(
                    bot,
                    &current,
                    &bot.resources.credentials,
                    None,
                    bot.state == BotState::Active,
                )
                .await?;
            if !restored.uses(&bot.resources.credentials) {
                return Err(BotError::Unavailable);
            }
            bot.resources.channel = Some(restored.channel);
            self.settled(bot).await?;
            if bot.state != BotState::Disabled && bot.request.connection_ref.is_none() {
                self.effect(bot, "restore_retained_route_key").await?;
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
            }
            self.retire_unused_keys(bot, &bot.resources.credentials.clone())
                .await?;
            bot.resources.replacement = Some(BotCredentialSet::default());
            bot.generation = bot.generation.checked_add(1).ok_or(BotError::Conflict)?;
            bot.work.as_mut().ok_or(BotError::Conflict)?.generation = bot.generation;
            self.step(bot, "replacement_reset").await?;
        }
        let secrets = self.mint_pair(bot, true).await?;
        let replacement = bot
            .resources
            .replacement
            .clone()
            .ok_or(BotError::Conflict)?;
        let current = self
            .external
            .channel(bot)
            .await?
            .ok_or(BotError::Unavailable)?;
        if bot.request.connection_ref.is_none() {
            self.effect(bot, "set_replacement_callback").await?;
            self.external
                .set_callback(
                    bot,
                    replacement
                        .route_key_id
                        .as_deref()
                        .ok_or(BotError::Unavailable)?,
                    &current.channel.callback_url,
                )
                .await?;
            self.settled(bot).await?;
        }
        self.effect(bot, "replace_channel_keys").await?;
        let changed = self
            .external
            .configure_channel(
                bot,
                &current,
                &replacement,
                Some(&secrets),
                bot.state == BotState::Active,
            )
            .await?;
        if !changed.uses(&replacement) {
            return Err(BotError::Unavailable);
        }
        bot.resources.channel = Some(changed.channel.clone());
        self.settled(bot).await?;
        self.finish_rotation(bot, changed, replacement).await
    }
    async fn finish_rotation(
        &self,
        bot: &mut Bot,
        current: BotChannelObservation,
        replacement: BotCredentialSet,
    ) -> Result<(), BotError> {
        // Reconcile even after a successful channel read: another interrupted
        // request may have reached the provider before failing its record write.
        self.effect(bot, "reconcile_replacement_binding").await?;
        let current = self
            .external
            .configure_channel(
                bot,
                &current,
                &replacement,
                None,
                bot.state == BotState::Active,
            )
            .await?;
        if !current.uses(&replacement) {
            return Err(BotError::Unavailable);
        }
        bot.resources.channel = Some(current.channel);
        self.settled(bot).await?;
        if bot.state != BotState::Disabled && bot.request.connection_ref.is_none() {
            self.effect(bot, "replace_route_key").await?;
            self.external
                .set_route_key(
                    bot,
                    bot.resources
                        .route_id
                        .as_deref()
                        .ok_or(BotError::Unavailable)?,
                    replacement
                        .route_key_id
                        .as_deref()
                        .ok_or(BotError::Unavailable)?,
                )
                .await?;
            self.settled(bot).await?;
        }
        self.retire_unused_keys(bot, &replacement).await?;
        bot.resources.credentials = replacement;
        bot.resources.replacement = None;
        self.service
            .complete(bot, &self.holder, bot.state.clone())
            .await
    }
    pub(super) async fn retire_unused_keys(
        &self,
        bot: &mut Bot,
        retained: &BotCredentialSet,
    ) -> Result<(), BotError> {
        let active_route_key =
            if bot.state == BotState::Disabled && bot.request.connection_ref.is_none() {
                self.step(bot, "read_retained_route_key").await?;
                self.external.active_route_key(bot).await?
            } else {
                None
            };
        self.step(bot, "discover_obsolete_keys").await?;
        let keys = self.external.keys(bot).await?;
        for key in keys {
            if Some(key.id.as_str()) != retained.agent_key_id.as_deref()
                && Some(key.id.as_str()) != retained.route_key_id.as_deref()
                && Some(key.id.as_str()) != active_route_key.as_deref()
            {
                self.effect(bot, "revoke_obsolete_key").await?;
                self.external.revoke(bot, &key.id).await?;
                self.settled(bot).await?;
            }
        }
        Ok(())
    }
}
