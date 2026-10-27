//! Card preparation keeps native forms an explicit authority decision.
use super::*;
use crate::providers::EventContextSnapshot;
use cmaeg_core::{message::OutboundMessage, seal::ReplyAddress};

impl InteractionStore {
    /// Prepare a card for `event`, owned by the consumer the event was issued
    /// to: its context is read through that consumer's binding and
    /// credential, and its clicks are bound to that context's actor, chat and
    /// thread.
    pub async fn prepare(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        let context = self.event_context(channel, event).await?;
        self.prepare_with_context(channel, event, message, context)
            .await
    }
    /// Read once before selecting a native representation. Invalid authority is
    /// an error, never a reason to fall back to another delivery path.
    pub(crate) async fn event_context(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
    ) -> Result<EventContextSnapshot, Error> {
        let consumer = channel
            .consumer(event.consumer.as_ref())
            .ok_or(Error::Unavailable)?;
        let credential = consumer.credential.ok_or(Error::Unavailable)?;
        let context = self
            .providers
            .event_context(
                &consumer.provider.slug,
                &consumer.provider.binding_id,
                &event.conversation_id,
                &event.event_id,
                credential,
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        if context.channel_id != channel.id || !event.verifies_activity(&context.activity) {
            return Err(Error::Denied);
        }
        Ok(context)
    }
    /// Reuse the verified preflight snapshot without a second provider request.
    /// The final binding check also applies to the existing public prepare path.
    pub(crate) async fn prepare_with_context(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        context: EventContextSnapshot,
    ) -> Result<PreparedCard, Error> {
        self.prepare_with_discord_thread(channel, event, message, context, None)
            .await
    }
    /// Thread controls require fresh authenticated channel evidence in addition
    /// to the sealed event's retained existing-thread destination.
    pub(crate) async fn prepare_with_discord_thread(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        context: EventContextSnapshot,
        thread: Option<crate::discord::component_thread::ThreadIdentity>,
    ) -> Result<PreparedCard, Error> {
        self.prepare_card(channel, event, message, context, thread, false)
            .await
    }
    /// Explicit issuance path: legacy preparation never grants native form authority.
    pub async fn prepare_lark_form(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        let context = self.event_context(channel, event).await?;
        self.prepare_lark_form_with_context(channel, event, message, context)
            .await
    }
    pub(crate) async fn prepare_lark_form_with_context(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        context: EventContextSnapshot,
    ) -> Result<PreparedCard, Error> {
        if !matches!(event.reply, ReplyAddress::NyxidRelay { platform, .. } if lark::supported(platform))
        {
            return Err(Error::Denied);
        }
        lark_form::schema(message)?;
        self.prepare_card(channel, event, message, context, None, true)
            .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn prepare_card(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        context: EventContextSnapshot,
        thread: Option<crate::discord::component_thread::ThreadIdentity>,
        native_lark_form: bool,
    ) -> Result<PreparedCard, Error> {
        if context.channel_id != channel.id
            || !event.verifies_activity(&context.activity)
            || (message.thread_reply
                && matches!(event.reply, ReplyAddress::Discord { .. })
                && thread.is_none())
            || (thread.is_some() && !matches!(event.reply, ReplyAddress::Discord { .. }))
        {
            return Err(Error::Denied);
        }
        let route_id = match &event.reply {
            ReplyAddress::NyxidRelay { route_id, .. } => route_id.clone(),
            ReplyAddress::Telegram {
                bot_id, chat_id, ..
            } => crate::telegram::route(bot_id, chat_id),
            ReplyAddress::Discord {
                bot_id, channel_id, ..
            } => discord::route(bot_id, channel_id),
            _ => return Err(Error::Denied),
        };
        let lark = match &event.reply {
            ReplyAddress::NyxidRelay { platform, .. } if lark::supported(*platform) => {
                if context.activity.actor.kind != cmaeg_core::activity::ActorKind::Human
                    || context.activity.conversation.thread_id.is_some()
                    || context.activity.thread.is_some()
                {
                    return Err(Error::Denied);
                }
                Some(LarkIdentity {
                    platform: *platform,
                    conversation: context.activity.conversation.kind,
                    form: if native_lark_form {
                        lark_form_authority::FormAuthority::prepare(
                            channel,
                            event,
                            &context.activity,
                            message,
                        )?
                    } else {
                        None
                    },
                })
            }
            _ => None,
        };
        let whatsapp = if matches!(
            event.reply,
            ReplyAddress::NyxidRelay {
                platform: cmaeg_core::channel::Platform::Whatsapp,
                ..
            }
        ) {
            if !whatsapp::eligible(channel, event, &context.activity) {
                return Err(Error::Denied);
            }
            let (markup, _) = whatsapp::render(message, "00000000000000000000000000000000", 1)?;
            Some(if markup["type"] == "button" {
                WhatsappKind::Button
            } else {
                WhatsappKind::List
            })
        } else {
            None
        };
        let card = Card {
            discord_edit: None,
            discord: if matches!(event.reply, ReplyAddress::Discord { .. }) {
                Some(discord::identity(
                    channel,
                    event,
                    &context.activity,
                    thread,
                )?)
            } else {
                None
            },
            callback_owner: owner::prepare(channel, event, &context.activity)?,
            lark,
            whatsapp,
            id: uuid::Uuid::new_v4().to_string(),
            owner: channel.definition.owner.subject.clone(),
            channel_id: channel.id.clone(),
            event_id: event.event_id.clone(),
            route_id: route_id.clone(),
            actor: context.activity.actor.id,
            chat: context.activity.conversation.id,
            thread: context.activity.conversation.thread_id,
            expires_at: now() + TTL,
            revision: 1,
            message: message.clone(),
            destination: None,
            reference: None,
            consumed: None,
            superseded: false,
            // A reference never names the default, so a default card keeps
            // its exact stored bytes.
            consumer: event.consumer.clone(),
        };
        let markup = markup(&card)?;
        let version = self.save(&card, None).await?;
        Ok(PreparedCard {
            card,
            version,
            markup,
        })
    }
}
