//! Callback authorization and durable consumption, shared by native and relay ingress.
use super::{card::*, keyboard::actions, lark, owner, Admit, InteractionStore};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::activity::{Activity, InteractionResult};
use serde_json::json;

impl InteractionStore {
    /// Consume a click on a card. `admit` decides, from the card's consumer,
    /// whether the click may be routed at all; it runs after the actor, chat
    /// and thread checks and before `consumed` is written, so a refused click
    /// leaves the card clickable.
    // Each argument is one fact of the click the card is checked against;
    // bundling them would only rename them at the one production caller.
    #[allow(clippy::too_many_arguments)]
    async fn consume_card(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        route: &str,
        callback_id: &str,
        data: &str,
        platform_message_id: &str,
        submitted: Option<&serde_json::Map<String, serde_json::Value>>,
        admit: &Admit,
    ) -> Result<(InteractionResult, String, Card), Error> {
        let (id, index, revision) = decode(data)?;
        let mut retry_contention = false;
        for _ in 0..2 {
            let (mut card, version) = self.load(&id).await?;
            retry_contention = card.native_telegram_media.is_some();
            let platform_matches = if let Some(kind) = card.whatsapp {
                data.starts_with(kind.prefix())
                    && revision == Some(card.revision)
                    && (if let Some(authority) = &card.native_whatsapp {
                        authority.current(&card, channel, activity)
                    } else {
                        matches!(
                            &activity.source,
                            cmaeg_core::activity::SourceKind::NyxidRelay {
                                platform: cmaeg_core::channel::Platform::Whatsapp,
                                route_id
                            } if route_id == route
                        )
                    })
                    && activity.conversation.kind == cmaeg_core::activity::ConversationKind::Private
                    && activity.thread.is_none()
            } else {
                match &card.lark {
                    Some(lark) => {
                        data.starts_with(if lark.form.is_some() { "clf1:" } else { "cl1:" })
                            && revision == Some(card.revision)
                            && matches!(activity.source, cmaeg_core::activity::SourceKind::NyxidRelay { platform, .. } if platform == lark.platform)
                            && activity.conversation.kind == lark.conversation
                    }
                    None => {
                        data.starts_with("ce1:")
                            && revision.is_none()
                            && !matches!(activity.source, cmaeg_core::activity::SourceKind::NyxidRelay { platform, .. } if lark::supported(platform) || platform == cmaeg_core::channel::Platform::Whatsapp)
                    }
                }
            };
            if card.channel_id != channel.id
                || card.discord.is_some()
                || card.native_lark.is_some()
                || !owner::admitted(&card, channel, activity)
                || card
                    .native_telegram_media
                    .as_ref()
                    .is_some_and(|a| !a.admitted(&card, channel, activity))
                || !platform_matches
                || card.route_id != route
                || card.actor != activity.actor.id
                || card.chat != activity.conversation.id
                || card.thread != activity.conversation.thread_id
                || (card.superseded
                    && !super::lark_effect::recovery(
                        &card,
                        activity.event_id.as_str(),
                        callback_id,
                        index,
                    ))
            {
                return Err(Error::Denied);
            }
            // A native callback can race the confirmed-send binding. Preserve its
            // retry opportunity, but never consume an unbound/uncertain card.
            if card.reference.is_none() || card.destination.is_none() {
                return Err(
                    if card.native_whatsapp.is_some() || card.native_telegram_media.is_some() {
                        Error::Unavailable
                    } else {
                        Error::Denied
                    },
                );
            }
            if card
                .destination
                .as_ref()
                .is_none_or(|d| d.platform_id != platform_message_id)
            {
                return Err(Error::Denied);
            }
            if !admit(card.consumer.as_ref()) {
                tracing::info!(card = %id, "card click not admitted for its consumer");
                return Err(Error::Denied);
            }
            let form = card
                .lark
                .as_ref()
                .and_then(|identity| identity.form.as_ref());
            let (action, submitted) = if let Some(form) = form {
                if !form.current(&card, channel, activity) {
                    return Err(Error::Denied);
                }
                let values = super::lark_form::submitted(
                    &card.message,
                    index,
                    submitted.ok_or(Error::Denied)?,
                )?;
                let (_, id, _) = super::lark_form::schema(&card.message)?;
                (
                    Action {
                        block: id.into(),
                        id: id.into(),
                        selected: vec![],
                    },
                    Some(values),
                )
            } else {
                if submitted.is_some() {
                    return Err(Error::Denied);
                }
                (
                    actions(&card.message)
                        .get(index)
                        .cloned()
                        .ok_or(Error::Denied)?,
                    None,
                )
            };
            if let Some(click) = &card.consumed {
                if click.event_id != activity.event_id.as_str()
                    || click.callback_id != callback_id
                    || click.action != index
                    || click.submitted != submitted
                {
                    return Err(Error::Denied);
                }
            } else {
                card.consumed = Some(Click {
                    form_launch: None,
                    event_id: activity.event_id.to_string(),
                    callback_id: callback_id.into(),
                    action: index,
                    selection: None,
                    submitted,
                    occurred_at: activity.occurred_at.clone(),
                });
                match self.save(&card, Some(version)).await {
                    Ok(_) => {}
                    Err(Error::Denied) => continue,
                    Err(e) => return Err(e),
                }
            }
            // Reference sealing is randomized. Retrying the same click must carry identical context.
            let destination = card.destination.as_ref().ok_or(Error::Denied)?;
            let mut values = json!({"message_ref":card.reference,"interaction_record_id":card.id,"revision":card.revision,"platform_message_id":destination.platform_id});
            if let Some(submitted) = card
                .consumed
                .as_ref()
                .and_then(|click| click.submitted.as_ref())
            {
                values["fields"] = serde_json::to_value(submitted).map_err(|_| Error::Denied)?;
            } else {
                values["selected"] = json!(action.selected);
            }
            return Ok((
                InteractionResult {
                    block_id: action.block,
                    action_id: action.id,
                    values: values.as_object().unwrap().clone(),
                },
                card.consumed.as_ref().unwrap().occurred_at.clone(),
                card,
            ));
        }
        // Repeated CAS contention has not established refusal. Let the relay
        // retry a native media, Lark or WhatsApp click instead of dropping it.
        Err(
            if retry_contention
                || data.starts_with("cl1:")
                || data.starts_with("clf1:")
                || data.starts_with("cw1b:")
                || data.starts_with("cw1l:")
            {
                Error::Unavailable
            } else {
                Error::Denied
            },
        )
    }
    /// Retained caller API; production also retrieves the sealed owner binding.
    #[allow(clippy::too_many_arguments)]
    pub async fn consume(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        route: &str,
        callback_id: &str,
        data: &str,
        platform_message_id: &str,
        admit: &Admit,
    ) -> Result<(InteractionResult, String), Error> {
        self.consume_card(
            channel,
            activity,
            route,
            callback_id,
            data,
            platform_message_id,
            None,
            admit,
        )
        .await
        .map(|(result, time, _)| (result, time))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn consume_for_dispatch(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        route: &str,
        callback_id: &str,
        data: &str,
        platform_message_id: &str,
        submitted: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<
        (
            InteractionResult,
            String,
            Option<super::owner::DispatchOwner>,
        ),
        Error,
    > {
        self.consume_card(
            channel,
            activity,
            route,
            callback_id,
            data,
            platform_message_id,
            submitted,
            &|_| true,
        )
        .await
        .map(|(result, time, card)| {
            let owner = card
                .native_telegram_media
                .as_ref()
                .map(|a| a.owner.clone())
                .or_else(|| {
                    card.native_whatsapp
                        .as_ref()
                        .map(|authority| authority.owner.clone())
                })
                .or_else(|| {
                    card.lark
                        .as_ref()
                        .and_then(|identity| identity.form.as_ref())
                        .map(|form| form.owner.clone())
                })
                .or_else(|| card.callback_owner.map(Into::into));
            (result, time, owner)
        })
    }
}
