//! Form launch and submission compete with edit/delete on the original card CAS.
use super::{
    card::{now, Click},
    discord,
    discord_form::{self, Launch},
    Card, Error, InteractionStore,
};
use crate::{channels::resolver::ResolvedChannel, discord::component_thread::ThreadIdentity};
use cmaeg_core::activity::{Activity, InteractionResult, SourceKind};
use serde_json::{json, Value};

pub(crate) struct Callback<'a> {
    pub interaction: &'a str,
    pub custom: &'a str,
    pub message: &'a str,
    pub guild: Option<&'a str>,
    pub thread: Option<&'a ThreadIdentity>,
}
fn current(
    card: &Card,
    channel: &ResolvedChannel,
    activity: &Activity,
    callback: &Callback<'_>,
) -> bool {
    let Some(identity) = card.discord.as_ref() else {
        return false;
    };
    identity.form.is_some()
        && card.channel_id == channel.id
        && card.owner == channel.definition.owner.subject
        && card.actor == activity.actor.id
        && card.chat == activity.conversation.id
        && card.thread == activity.conversation.thread_id
        && identity.thread.as_ref() == callback.thread
        && activity.thread.is_none()
        && identity.conversation_kind == activity.conversation.kind
        && activity.source
            == (SourceKind::Discord {
                bot_id: identity.bot_id.clone(),
            })
        && card.reference.is_some()
        && card.expires_at > now()
        && card
            .destination
            .as_ref()
            .is_some_and(|d| d.platform_id == callback.message)
        && discord::source_revision(&channel.definition).ok().as_ref()
            == Some(&identity.source_revision)
        && channel
            .consumer(Some(&identity.consumer))
            .is_some_and(|consumer| {
                !consumer.pending
                    && consumer.credential.is_some()
                    && consumer.provider == &identity.provider
            })
}
impl InteractionStore {
    pub(crate) async fn launch_discord_form(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        callback: &Callback<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<Card, Error> {
        let (id, revision, block) = discord_form::decode(callback.custom, "cdf1")?;
        for _ in 0..3 {
            let (mut card, version) = self.load(&id).await?;
            if !current(&card, channel, activity, callback)
                || card.superseded
                || card.consumed.is_some()
                || revision != card.revision
                || block != discord_form::schema(&card.message)?.0.to_string()
            {
                return Err(Error::Denied);
            }
            if let Some(launch) = &card.discord_form_launch {
                let old = launch
                    .interaction
                    .parse::<u64>()
                    .map_err(|_| Error::Denied)?;
                let next = callback
                    .interaction
                    .parse::<u64>()
                    .map_err(|_| Error::Denied)?;
                if old > next {
                    return Err(Error::Denied);
                }
                if old == next {
                    return (launch.guild.as_deref() == callback.guild)
                        .then_some(card)
                        .ok_or(Error::Denied);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Denied);
            }
            card.discord_form_launch = Some(Launch {
                interaction: callback.interaction.into(),
                guild: callback.guild.map(str::to_owned),
                expires_at: card.expires_at.min(now() + 600),
                dispatched: false,
            });
            match self.save(&card, Some(version)).await {
                Ok(_) => return Ok(card),
                Err(Error::Denied) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Unavailable)
    }
    /// A successful CAS is the sole permission to send type9. A lost CAS response
    /// never retries the write; a fresh user launch still has independent authority.
    pub(crate) async fn claim_discord_modal(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        callback: &Callback<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<Option<Card>, Error> {
        let (id, revision, _) = discord_form::decode(callback.custom, "cdf1")?;
        let (mut card, version) = self.load(&id).await?;
        if !current(&card, channel, activity, callback)
            || card.superseded
            || card.consumed.is_some()
            || revision != card.revision
            || tokio::time::Instant::now() >= deadline
        {
            return Err(Error::Denied);
        }
        let launch = card.discord_form_launch.as_mut().ok_or(Error::Denied)?;
        if launch.interaction != callback.interaction || launch.guild.as_deref() != callback.guild {
            return Err(Error::Denied);
        }
        if launch.dispatched {
            return Ok(None);
        }
        launch.dispatched = true;
        self.save(&card, Some(version)).await?;
        Ok(Some(card))
    }
    /// None is an authenticated invalid field set. It never consumes the card.
    pub(crate) async fn submit_discord_form(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        callback: &Callback<'_>,
        components: &Value,
        deadline: tokio::time::Instant,
    ) -> Result<Option<(InteractionResult, Card)>, Error> {
        let (id, revision, attempt) = discord_form::decode(callback.custom, "cdm1")?;
        for _ in 0..3 {
            let (mut card, version) = self.load(&id).await?;
            if !current(&card, channel, activity, callback) || revision != card.revision {
                return Err(Error::Denied);
            }
            let launch = card.discord_form_launch.as_ref().ok_or(Error::Denied)?;
            if launch.interaction != attempt
                || !launch.dispatched
                || launch.guild.as_deref() != callback.guild
                || (card.superseded
                    && (card.consumed.is_none()
                        || (card.discord_edit.is_none()
                            && !super::discord_effect::deleting(&card))))
            {
                return Err(Error::Denied);
            }
            let submitted = match super::discord_form_values::submitted(&card.message, components) {
                Ok(values) => values,
                Err(_) => return Ok(None),
            };
            let block = discord_form::schema(&card.message)?.0;
            if let Some(click) = &card.consumed {
                if click.event_id != activity.event_id.as_str()
                    || click.callback_id != callback.interaction
                    || click.action != block
                    || click.form_launch.as_ref() != Some(&attempt)
                    || click.submitted.as_ref() != Some(&submitted)
                {
                    return Err(Error::Denied);
                }
            } else {
                if now() >= launch.expires_at || tokio::time::Instant::now() >= deadline {
                    return Err(Error::Denied);
                }
                card.consumed = Some(Click {
                    event_id: activity.event_id.to_string(),
                    callback_id: callback.interaction.into(),
                    action: block,
                    selection: None,
                    submitted: Some(submitted),
                    form_launch: Some(attempt.clone()),
                    occurred_at: activity.occurred_at.clone(),
                });
                match self.save(&card, Some(version)).await {
                    Ok(_) => {}
                    Err(Error::Denied) => continue,
                    Err(error) => return Err(error),
                }
            }
            let name = discord_form::schema(&card.message)?.1.to_owned();
            let values = json!({"fields":card.consumed.as_ref().and_then(|click| click.submitted.as_ref()),
                "message_ref":card.reference,"interaction_record_id":card.id,"revision":card.revision,"platform_message_id":callback.message});
            return Ok(Some((
                InteractionResult {
                    block_id: name.clone(),
                    action_id: name,
                    values: values.as_object().unwrap().clone(),
                },
                card,
            )));
        }
        Err(Error::Unavailable)
    }
}
