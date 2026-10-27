//! Native component consumption reuses encrypted cards and their single-winner CAS.
use super::{
    card::{now, Click},
    discord, Card, Error, InteractionStore,
};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::activity::{Activity, InteractionResult};
use serde_json::json;

pub(crate) struct DiscordClick<'a> {
    pub id: &'a str,
    pub custom_id: &'a str,
    pub component_type: u64,
    pub values: &'a [String],
    pub message: &'a str,
    pub thread: Option<&'a crate::discord::component_thread::ThreadIdentity>,
}
impl InteractionStore {
    pub(crate) async fn discord_thread(
        &self,
        custom: &str,
    ) -> Result<
        (
            Option<crate::discord::component_thread::ThreadIdentity>,
            bool,
        ),
        Error,
    > {
        let (id, _, _) = super::card::decode(custom)?;
        let (card, _) = self.load(&id).await?;
        let identity = card.discord.ok_or(Error::Denied)?;
        Ok((identity.thread, card.consumed.is_some()))
    }

    pub(crate) async fn consume_discord(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        click: DiscordClick<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<(InteractionResult, Card), Error> {
        let (id, _, _) = super::card::decode(click.custom_id)?;
        for _ in 0..2 {
            let (mut card, version) = self.load(&id).await?;
            let identity = card.discord.as_ref().ok_or(Error::Denied)?;
            let resolved =
                discord::action(&card, click.custom_id, click.component_type, click.values)?;
            if card.channel_id != channel.id
                || card.owner != channel.definition.owner.subject
                || card.actor != activity.actor.id
                || card.chat != activity.conversation.id
                || card.thread != activity.conversation.thread_id
                || identity.thread.as_ref() != click.thread
                || activity.thread.is_some()
                || identity.conversation_kind != activity.conversation.kind
                || activity.source
                    != (cmaeg_core::activity::SourceKind::Discord {
                        bot_id: identity.bot_id.clone(),
                    })
                || (card.superseded && (card.discord_edit.is_none() || card.consumed.is_none()))
                || card.reference.is_none()
                || card.expires_at <= now()
                || card
                    .destination
                    .as_ref()
                    .is_none_or(|d| d.platform_id != click.message)
                || discord::source_revision(&channel.definition)? != identity.source_revision
                || channel
                    .consumer(Some(&identity.consumer))
                    .is_none_or(|current| {
                        current.pending
                            || current.credential.is_none()
                            || current.provider != &identity.provider
                    })
            {
                return Err(Error::Denied);
            }
            let action = resolved.action;
            if let Some(consumed) = &card.consumed {
                if consumed.event_id != activity.event_id.as_str()
                    || consumed.callback_id != click.id
                    || consumed.action != resolved.index
                    || consumed.selection != resolved.selection
                {
                    return Err(Error::Denied);
                }
            } else {
                // Exact consumed clicks can finish their inbox after the ACK
                // deadline; a new late click cannot consume fresh authority.
                if tokio::time::Instant::now() >= deadline {
                    return Err(Error::Denied);
                }
                card.consumed = Some(Click {
                    event_id: activity.event_id.to_string(),
                    callback_id: click.id.into(),
                    action: resolved.index,
                    selection: resolved.selection,
                    submitted: None,
                    occurred_at: activity.occurred_at.clone(),
                });
                match self.save(&card, Some(version)).await {
                    Ok(_) => {}
                    Err(Error::Denied) => continue,
                    Err(error) => return Err(error),
                }
            }
            let values = json!({"selected":action.selected,"message_ref":card.reference,"interaction_record_id":card.id,"revision":card.revision,"platform_message_id":click.message});
            return Ok((
                InteractionResult {
                    block_id: action.block,
                    action_id: action.id,
                    values: values.as_object().unwrap().clone(),
                },
                card,
            ));
        }
        Err(Error::Unavailable)
    }
}
