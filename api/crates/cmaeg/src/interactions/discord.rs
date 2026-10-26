//! Native Discord controls carry opaque card/revision/action locators only.
use super::{Card, Error};
use crate::channels::resolver::ResolvedChannel;
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
pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    event.lineage.is_none()
        && activity.actor.kind == ActorKind::Human
        && activity.thread.is_none()
        && activity.conversation.thread_id.is_none()
        && channel.definition.routing.is_some()
        && super::owner::eligible(channel, event, activity)
        && matches!((&event.reply, &activity.source),
            (ReplyAddress::Discord { bot_id, channel_id, .. }, SourceKind::Discord { bot_id: source })
            if bot_id == source && channel_id == &activity.conversation.id)
}
pub(super) fn identity(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> Result<Identity, Error> {
    if !eligible(channel, event, activity) {
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
        return true;
    };
    card.lark.is_none()
        && card.whatsapp.is_none()
        && card.thread.is_none()
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
        && render(&card.message, &card.id, card.revision).is_ok()
}
pub(crate) fn route(bot: &str, channel: &str) -> String {
    format!("discord:{bot}:{channel}")
}
fn locator(id: &str, revision: u64, action: usize) -> String {
    format!("cd1:{}:{revision}:{action}", id.replace('-', ""))
}
/// At most five action rows; buttons are split into rows of five. A single-select
/// occupies one row. Neutral message bounds further limit selects to 20 options.
pub(crate) fn render(message: &OutboundMessage, id: &str, revision: u64) -> Result<Value, Error> {
    message.validate().map_err(|_| Error::Denied)?;
    if revision == 0
        || message.thread_reply
        || message.reply_to.is_some()
        || message.whatsapp_template.is_some()
        || !message.native_attachments.is_empty()
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
                if *multi || options.len() > 25 || options.iter().filter(|o| o.selected).count() > 1
                {
                    return Err(Error::Denied);
                }
                let start = action;
                let options: Vec<_> = options.iter().map(|option| {
                    let value = json!({"label":option.label,"value":action.to_string(),"default":option.selected});
                    action += 1; value
                }).collect();
                let placeholder: String = prompt.chars().take(100).collect();
                rows.push(json!({"type":1,"components":[{"type":3,"custom_id":locator(id,revision,start),"placeholder":placeholder,"min_values":1,"max_values":1,"options":options}]}));
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
/// Convert a selected opaque index to the matching immutable card action. Client
/// labels and business values never contribute callback authority.
pub(super) fn action(
    card: &Card,
    custom: &str,
    kind: u64,
    values: &[String],
) -> Result<usize, Error> {
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
                    return Ok(index);
                }
                start += options.len();
            }
            Block::Choice { options, multi, .. } => {
                if kind == 3 && !multi && index == start && values.len() == 1 {
                    let selected = values[0].parse::<usize>().map_err(|_| Error::Denied)?;
                    if selected.to_string() == values[0]
                        && (start..start + options.len()).contains(&selected)
                    {
                        return Ok(selected);
                    }
                }
                start += options.len();
            }
            _ => {}
        }
    }
    Err(Error::Denied)
}
