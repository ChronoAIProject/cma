//! Native Discord controls carry opaque card/revision/action locators only.
use super::{Card, Error};
use crate::{channels::resolver::ResolvedChannel, discord::component_thread::ThreadIdentity};
use cmaeg_core::{
    activity::{Activity, ActivityRouting, ActorKind, SourceKind},
    channel::ProviderBinding,
    ids::{ConsumerId, ConversationId},
    message::{Block, OptionStyle, OutboundMessage},
    routing::{Resolution, RouteInput},
    seal::{EventRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Same-message upload fact. Issuance requires the media-controls reader floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<super::discord_media::Media>,
    /// Ordered facts for two to four uploads; mutually exclusive with legacy media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_set: Option<Vec<super::discord_media::Media>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<super::discord_form::Authority>,
    pub bot_id: String,
    pub source_revision: String,
    pub consumer: ConsumerId,
    pub provider: ProviderBinding,
    pub conversation_id: ConversationId,
    pub partition_key: String,
    pub routing: ActivityRouting,
    pub conversation_kind: cmaeg_core::activity::ConversationKind,
}
pub(crate) fn requested(message: &OutboundMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Buttons { .. } | Block::Choice { .. }))
}
pub(crate) fn multiselect_requested(message: &OutboundMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Choice { multi: true, .. }))
}
pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    event.lineage.is_none()
        && activity.actor.kind == ActorKind::Human
        && activity.thread.is_none()
        && channel.definition.routing.is_some()
        && super::owner::eligible(channel, event, activity)
        && matches!((&event.reply, &activity.source),
            (ReplyAddress::Discord { bot_id, channel_id, thread_id, .. }, SourceKind::Discord { bot_id: source })
            if bot_id == source && channel_id == &activity.conversation.id
                && thread_id == &activity.conversation.thread_id
                && thread_id.as_ref().is_none_or(|id| id == channel_id))
}
pub(super) fn identity(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
    thread: Option<ThreadIdentity>,
) -> Result<Identity, Error> {
    if !eligible(channel, event, activity)
        || !ThreadIdentity::optional_matches(thread.as_ref(), &activity.conversation)
    {
        return Err(Error::Denied);
    }
    let SourceKind::Discord { bot_id } = &activity.source else {
        return Err(Error::Denied);
    };
    let consumer = channel
        .consumer(event.consumer.as_ref())
        .ok_or(Error::Denied)?;
    let input = RouteInput {
        activity,
        chat_source: channel.definition.source_of(&activity.source),
        card_consumer: Some(event.consumer.as_ref()),
        published: None,
        non_human_providers: &Default::default(),
        attributes_signed: true,
    };
    let Resolution::Routed(decision) =
        cmaeg_core::routing::resolve(&channel.definition, &input, true)
            .map_err(|_| Error::Denied)?
    else {
        return Err(Error::Denied);
    };
    Ok(Identity {
        media: None,
        media_set: None,
        form: None,
        thread,
        bot_id: bot_id.clone(),
        source_revision: source_revision(&channel.definition)?,
        consumer: consumer.id.clone(),
        provider: consumer.provider.clone(),
        conversation_id: event.conversation_id.clone(),
        partition_key: event.partition_key.clone(),
        routing: decision.routing,
        conversation_kind: activity.conversation.kind,
    })
}
pub(crate) fn source_revision(
    definition: &cmaeg_core::channel::ChannelDefinition,
) -> Result<String, Error> {
    crate::discord::native_source_revision(definition).ok_or(Error::Denied)
}
pub(crate) fn valid(card: &Card) -> bool {
    let Some(identity) = &card.discord else {
        return card
            .consumed
            .as_ref()
            .is_none_or(|click| click.selection.is_none());
    };
    card.message.native_attachments.is_empty()
        && !card.message.clear_native_attachments
        && identity.media_items().is_ok_and(|items| {
            items
                .iter()
                .all(|media| media.valid_for(card) && identity.form.is_none())
        })
        && card.native_whatsapp_flow.is_none()
        && card.lark.is_none()
        && card.whatsapp.is_none()
        && ThreadIdentity::optional_matches(
            identity.thread.as_ref(),
            &cmaeg_core::activity::ActivityConversation {
                id: card.chat.clone(),
                kind: identity.conversation_kind,
                thread_id: card.thread.clone(),
            },
        )
        && (!card.message.thread_reply || card.thread.is_some())
        && card.revision > 0
        && card.consumer.as_ref() == (!identity.consumer.is_default()).then_some(&identity.consumer)
        && identity.routing.resolution == cmaeg_core::activity::RouteResolution::Interaction
        && identity.routing.targets.len() == 1
        && identity.routing.targets[0].id == identity.consumer
        && identity.routing.source_id
            == (SourceKind::Discord {
                bot_id: identity.bot_id.clone(),
            })
            .source_id()
        && identity.routing.causation.is_none()
        && identity.routing.rule_id.is_none()
        && identity.routing.addressing_bytes == 0
        && !identity.partition_key.is_empty()
        && identity.partition_key.len() <= 4096
        && cmaeg_core::channel::valid_discord_id(&identity.bot_id)
        && card.route_id == route(&identity.bot_id, &card.chat)
        && (if identity.form.is_some() {
            super::discord_form::valid(card)
        } else {
            (if card.revision > 1 {
                render_replacement
            } else {
                render
            })(&card.message, &card.id, card.revision)
            .is_ok()
                && selection_valid(card)
        })
}
pub(crate) fn route(bot: &str, channel: &str) -> String {
    format!("discord:{bot}:{channel}")
}
fn locator(id: &str, revision: u64, action: usize) -> String {
    format!("cd1:{}:{revision}:{action}", id.replace('-', ""))
}
/// At most five action rows; buttons are split into rows of five. A select
/// occupies one row. Neutral message bounds further limit selects to 20 options.
pub(crate) fn render(message: &OutboundMessage, id: &str, revision: u64) -> Result<Value, Error> {
    message.validate().map_err(|_| Error::Denied)?;
    if revision == 0
        || message.reply_to.is_some()
        || message.whatsapp_template.is_some()
        || message.clear_native_attachments
        || message.native_attachments.len() > cmaeg_core::message::MAX_NATIVE_ATTACHMENTS
        || (!message.native_attachments.is_empty()
            && message
                .blocks
                .iter()
                .any(|block| matches!(block, Block::Form { .. })))
    {
        return Err(Error::Denied);
    }
    let mut rows = Vec::new();
    let mut action = 0;
    for block in &message.blocks {
        match block {
            Block::Buttons { options, .. } => {
                for group in options.chunks(5) {
                    let mut buttons = Vec::new();
                    for option in group {
                        let style = match option.style {
                            OptionStyle::Default => 2,
                            OptionStyle::Primary => 1,
                            OptionStyle::Danger => 4,
                        };
                        buttons.push(json!({"type":2,"style":style,"label":option.label,"custom_id":locator(id,revision,action)}));
                        action += 1;
                    }
                    rows.push(json!({"type":1,"components":buttons}));
                }
            }
            Block::Choice {
                options,
                prompt,
                multi,
                ..
            } => {
                if options.len() > 25
                    || (!multi && options.iter().filter(|o| o.selected).count() > 1)
                {
                    return Err(Error::Denied);
                }
                let start = action;
                let maximum = if *multi { options.len() } else { 1 };
                let options: Vec<_> = options.iter().map(|option| {
                    let value = json!({"label":option.label,"value":action.to_string(),"default":option.selected});
                    action += 1; value
                }).collect();
                let placeholder: String = prompt.chars().take(100).collect();
                rows.push(json!({"type":1,"components":[{"type":3,"custom_id":locator(id,revision,start),"placeholder":placeholder,"min_values":if *multi { 0 } else { 1 },"max_values":maximum,"options":options}]}));
            }
            // These retain the ordinary text renderer's semantics; only controls
            // become components, so links do not introduce another callback kind.
            _ => {}
        }
    }
    if rows.is_empty() || rows.len() > 5 {
        return Err(Error::Denied);
    }
    Ok(Value::Array(rows))
}
/// A confirmed replacement can remove every control while retaining its exact
/// card reference for subsequent edits. Initial issuance still requires controls.
pub(crate) fn render_replacement(
    message: &OutboundMessage,
    id: &str,
    revision: u64,
) -> Result<Value, Error> {
    if !requested(message)
        && crate::delivery::discord_edit::editable_content(message)
        && message.reply_to.is_none()
        && revision > 1
    {
        Ok(json!([]))
    } else {
        render(message, id, revision)
    }
}
pub(super) struct CallbackAction {
    pub index: usize,
    pub selection: Option<Vec<usize>>,
    pub action: super::card::Action,
}
impl CallbackAction {
    fn single(card: &Card, index: usize) -> Result<Self, Error> {
        Ok(Self {
            index,
            selection: None,
            action: super::keyboard::actions(&card.message)
                .get(index)
                .cloned()
                .ok_or(Error::Denied)?,
        })
    }
}
/// Resolve opaque indices against one immutable block, never client business values.
pub(super) fn action(
    card: &Card,
    custom: &str,
    kind: u64,
    values: &[String],
) -> Result<CallbackAction, Error> {
    let (id, index, revision) = super::card::decode(custom)?;
    if !custom.starts_with("cd1:") || id != card.id || revision != Some(card.revision) {
        return Err(Error::Denied);
    }
    let mut start = 0;
    for block in &card.message.blocks {
        match block {
            Block::Buttons { options, .. } => {
                if kind == 2 && values.is_empty() && (start..start + options.len()).contains(&index)
                {
                    return CallbackAction::single(card, index);
                }
                start += options.len();
            }
            Block::Choice {
                id, options, multi, ..
            } => {
                if kind == 3
                    && index == start
                    && (*multi || values.len() == 1)
                    && values.len() <= options.len()
                {
                    let mut selected = values
                        .iter()
                        .map(|value| {
                            let selected = value.parse::<usize>().map_err(|_| Error::Denied)?;
                            (selected.to_string() == value.as_str()
                                && (start..start + options.len()).contains(&selected))
                            .then_some(selected)
                            .ok_or(Error::Denied)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    selected.sort_unstable();
                    if selected.windows(2).any(|pair| pair[0] == pair[1]) {
                        return Err(Error::Denied);
                    }
                    if !multi {
                        return CallbackAction::single(card, selected[0]);
                    }
                    return Ok(CallbackAction {
                        index: start,
                        action: super::card::Action {
                            block: id.clone(),
                            id: id.clone(),
                            selected: selected
                                .iter()
                                .map(|index| options[*index - start].id.clone())
                                .collect(),
                        },
                        selection: Some(selected),
                    });
                }
                start += options.len();
            }
            _ => {}
        }
    }
    Err(Error::Denied)
}

/// A stored multi click must preserve its complete set, including an empty set.
/// A single action index alone must never restore a different selection.
fn selection_valid(card: &Card) -> bool {
    let Some(click) = &card.consumed else {
        return true;
    };
    let mut start = 0;
    for block in &card.message.blocks {
        match block {
            Block::Buttons { options, .. } | Block::Choice { options, .. } => {
                let range = start..start + options.len();
                if range.contains(&click.action) {
                    return match block {
                        Block::Choice { multi: true, .. } => {
                            click.action == start
                                && click.selection.as_ref().is_some_and(|selection| {
                                    selection.len() <= options.len()
                                        && selection.iter().all(|index| range.contains(index))
                                        && selection.windows(2).all(|pair| pair[0] < pair[1])
                                })
                        }
                        _ => click.selection.is_none(),
                    };
                }
                start += options.len();
            }
            _ => {}
        }
    }
    false
}
