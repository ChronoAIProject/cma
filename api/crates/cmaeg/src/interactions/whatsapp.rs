//! Bounded reply buttons and single-choice lists for native and relay WhatsApp.
use super::{Card, Error, WhatsappKind};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, ActorKind, ConversationKind, SourceKind},
    channel::{Platform, Source},
    message::{
        Block, BlockRendering, BlockRenderingOutcome, OptionStyle, OutboundMessage,
        RenderingOutcome, RenderingReport,
    },
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) fn requested(message: &OutboundMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Buttons { .. } | Block::Choice { .. }))
}

/// A verified private sender is enough; Cloud API cannot attest that it is human.
pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    if matches!(event.reply, ReplyAddress::Whatsapp { .. }) {
        return event.lineage.is_none()
            && channel.definition.routing.is_some()
            && super::owner::eligible(channel, event, activity)
            && channel.definition.source_of(&activity.source).is_some()
            && crate::whatsapp::credentials(&channel.definition, &event.reply)
                .is_some_and(|credentials| credentials.validate().is_ok());
    }
    let ReplyAddress::NyxidRelay {
        platform: Platform::Whatsapp,
        route_id,
        thread_reply: false,
        ..
    } = &event.reply
    else {
        return false;
    };
    event.lineage.is_none()
        && activity.actor.kind != ActorKind::Bot
        && numeric(&activity.actor.id)
        && activity.actor.id == activity.conversation.id
        && activity.conversation.kind == ConversationKind::Private
        && activity.conversation.thread_id.is_none()
        && activity.thread.is_none()
        && matches!(&activity.source, SourceKind::NyxidRelay { platform: Platform::Whatsapp, route_id: source_route } if source_route == route_id)
        && matches!(
            channel.definition.relay_source_for_route(route_id),
            Some(Source::NyxidRelay {
                platform: Platform::Whatsapp,
                verify_raw_event: true,
                ..
            })
        )
}

pub(super) fn numeric(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_digit())
}

pub(super) fn valid_record(card: &Card) -> bool {
    let Some(kind) = card.whatsapp else {
        return card.native_whatsapp.is_none();
    };
    super::whatsapp_authority::valid(card)
        && card.discord.is_none()
        && card.lark.is_none()
        && card.thread.is_none()
        && card.revision > 0
        && numeric(&card.actor)
        && card.actor == card.chat
        && markup(card).is_ok()
        && matches!(
            (
                card.message
                    .blocks
                    .iter()
                    .find(|block| matches!(block, Block::Buttons { .. } | Block::Choice { .. })),
                kind
            ),
            (Some(Block::Buttons { .. }), WhatsappKind::Button)
                | (Some(Block::Choice { .. }), WhatsappKind::List)
        )
}

pub(crate) fn render(
    message: &OutboundMessage,
    id: &str,
    revision: u64,
) -> Result<(Value, RenderingReport), Error> {
    if message.validate().is_err()
        || message.whatsapp_template.is_some()
        || !message.native_attachments.is_empty()
        || message.thread_reply
        || revision == 0
    {
        return Err(Error::Denied);
    }
    let mut body = message.text.clone().unwrap_or_default();
    let mut control = None;
    let mut report = RenderingReport {
        outcome: RenderingOutcome::Rendered,
        blocks: vec![],
    };
    for (index, block) in message.blocks.iter().enumerate() {
        let mut degraded = false;
        match block {
            Block::Text { text } => append(&mut body, text),
            Block::Buttons { options, .. } | Block::Choice { options, .. } => {
                let kind = match block {
                    Block::Buttons { .. } => WhatsappKind::Button,
                    Block::Choice {
                        prompt,
                        multi: false,
                        ..
                    } => {
                        append(&mut body, prompt);
                        WhatsappKind::List
                    }
                    _ => return Err(Error::Denied),
                };
                if control.is_some()
                    || options.is_empty()
                    || options.len() > if kind == WhatsappKind::Button { 3 } else { 10 }
                {
                    return Err(Error::Denied);
                }
                let mut labels = HashSet::new();
                let mut values = Vec::new();
                for (action, option) in options.iter().enumerate() {
                    let label = format!(
                        "{}{}",
                        if option.selected { "✓ " } else { "" },
                        option.label
                    );
                    let limit = if kind == WhatsappKind::Button { 20 } else { 24 };
                    if label.trim().is_empty()
                        || label.trim() != label
                        || label.chars().any(char::is_control)
                        || label.chars().count() > limit
                        || !labels.insert(label.clone())
                    {
                        return Err(Error::Denied);
                    }
                    degraded |= option.style != OptionStyle::Default || option.selected;
                    let locator = format!(
                        "{}{}:{revision}:{action}",
                        kind.prefix(),
                        id.replace('-', "")
                    );
                    values.push(if kind == WhatsappKind::Button {
                        json!({"type":"reply","reply":{"id":locator,"title":label}})
                    } else {
                        json!({"id":locator,"title":label})
                    });
                }
                control = Some((kind, values));
            }
            // No partial delivery or implicit arbitrary metadata. Callers can
            // send a separate ordinary message for cards, forms, links or media.
            _ => return Err(Error::Denied),
        }
        let outcome = if degraded {
            report.outcome = RenderingOutcome::Degraded;
            BlockRenderingOutcome::Degraded {
                how: "plain_option_style".into(),
            }
        } else {
            BlockRenderingOutcome::Exact
        };
        report.blocks.push(BlockRendering {
            index: index as u16,
            outcome,
        });
    }
    let (kind, values) = control.ok_or(Error::Denied)?;
    if body.trim().is_empty() {
        body = "Choose an option".into();
    }
    // Conservative shared bound for both kinds, including older Graph versions.
    if body.chars().count() > 1024 {
        return Err(Error::Denied);
    }
    let markup = match kind {
        WhatsappKind::Button => {
            json!({"type":"button","body":{"text":body},"action":{"buttons":values}})
        }
        WhatsappKind::List => {
            json!({"type":"list","body":{"text":body},"action":{"button":"Choose an option","sections":[{"title":"Options","rows":values}]}})
        }
    };
    if serde_json::to_vec(&markup)
        .map_err(|_| Error::Denied)?
        .len()
        > 16 * 1024
    {
        return Err(Error::Denied);
    }
    Ok((markup, report))
}

fn append(body: &mut String, text: &str) {
    if !body.is_empty() && !text.is_empty() {
        body.push('\n');
    }
    body.push_str(text);
}

pub(super) fn markup(card: &Card) -> Result<Value, Error> {
    render(&card.message, &card.id, card.revision).map(|(markup, _)| markup)
}

#[cfg(test)]
#[path = "whatsapp_tests.rs"]
mod tests;
