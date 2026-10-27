//! Lark JSON 2.0 cards. Controls contain only a record/revision/action locator;
//! their values are resolved from CMA's encrypted record, never from the click.
use super::{card::decode, Card, Error, InteractionStore};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::ConversationKind,
    channel::Platform,
    message::{
        Block, BlockRendering, BlockRenderingOutcome, OptionStyle, OutboundMessage,
        RenderingOutcome, RenderingReport,
    },
};
use serde_json::{json, Value};

pub(crate) fn supported(platform: Platform) -> bool {
    matches!(platform, Platform::Lark | Platform::Feishu)
}

pub(crate) fn requested(message: &OutboundMessage) -> bool {
    super::has_controls(message)
        || message
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Card { .. }))
}

pub(crate) fn capabilities() -> crate::tools::Capabilities {
    crate::tools::Capabilities {
        buttons: "native_card".into(),
        cards: "native_card".into(),
        edit: true,
        ..crate::tools::Capabilities::plain_text()
    }
}

pub(crate) fn project(
    capabilities: &mut crate::tools::Capabilities,
    activity: &cmaeg_core::activity::Activity,
    event: &cmaeg_core::seal::EventRefPayload,
    verified: bool,
    enabled: bool,
) {
    use cmaeg_core::activity::{ActivityKind, ActorKind, SourceKind};
    if !verified
        || event.lineage.is_some()
        || activity.actor.kind != ActorKind::Human
        || activity.conversation.thread_id.is_some()
        || activity.thread.is_some()
        || !matches!(activity.source, SourceKind::NyxidRelay { platform, .. } if supported(platform))
    {
        return;
    }
    // An already issued card remains editable after operators stop new cards.
    if matches!(activity.kind, ActivityKind::Interaction(_)) {
        capabilities.edit = true;
    }
    if enabled {
        capabilities.buttons = "native_card".into();
        capabilities.cards = "native_card".into();
        capabilities.edit = true;
    }
}

/// The card is bounded before any durable write or platform send. Public forms
/// remain plain text; images require provider keys, so URL images stay links.
pub(crate) fn render(
    message: &OutboundMessage,
    id: &str,
    revision: u64,
) -> Result<(Value, RenderingReport), Error> {
    if message.whatsapp_template.is_some()
        || !message.native_attachments.is_empty()
        || message.thread_reply
        || revision == 0
    {
        return Err(Error::Denied);
    }
    let mut elements = Vec::new();
    let mut report = RenderingReport {
        outcome: RenderingOutcome::Rendered,
        blocks: vec![],
    };
    let mut action = 0;
    let mut text_chars = 0;
    if let Some(text) = &message.text {
        add_text(&mut elements, text, &mut text_chars);
    }
    for (index, block) in message.blocks.iter().enumerate() {
        let mut degraded = None;
        match block {
            Block::Text { text } => add_text(&mut elements, text, &mut text_chars),
            Block::Card {
                title,
                body,
                fields,
                image,
            } => {
                add_text(&mut elements, title, &mut text_chars);
                if let Some(body) = body {
                    add_text(&mut elements, body, &mut text_chars);
                }
                for field in fields {
                    add_text(
                        &mut elements,
                        &format!("{}: {}", field.label, field.value),
                        &mut text_chars,
                    );
                }
                if let Some(url) = image {
                    text_chars += "Open image".chars().count();
                    elements.push(link(url, "Open image"));
                    degraded = Some("image_link");
                }
            }
            Block::Buttons { options, .. } | Block::Choice { options, .. } => {
                if let Block::Choice { prompt, .. } = block {
                    add_text(&mut elements, prompt, &mut text_chars);
                }
                for option in options {
                    let label = format!(
                        "{}{}",
                        if option.selected { "✓ " } else { "" },
                        option.label
                    );
                    text_chars += label.chars().count();
                    let style = match option.style {
                        OptionStyle::Default => "default",
                        OptionStyle::Primary => "primary",
                        OptionStyle::Danger => "danger",
                    };
                    elements.push(json!({"tag":"button","type":style,
                    "text":{"tag":"plain_text","content":label},
                    "behaviors":[{"type":"callback","value":{
                        "cma_action":format!("cl1:{}:{revision}:{action}", id.replace('-', ""))
                    }}]}));
                    action += 1;
                }
            }
            Block::Link { url, label } => {
                let label = label.as_deref().unwrap_or("Open link");
                text_chars += label.chars().count();
                elements.push(link(url, label));
            }
            Block::Form { fields, .. } => {
                for field in fields {
                    add_text(
                        &mut elements,
                        &format!(
                            "{}:{}",
                            field.label,
                            if field.required { " (required)" } else { "" }
                        ),
                        &mut text_chars,
                    );
                }
                degraded = Some("plain_text_form");
            }
            Block::Attachment {
                reference, caption, ..
            } => {
                if let Some(caption) = caption {
                    add_text(&mut elements, caption, &mut text_chars);
                }
                add_text(&mut elements, reference, &mut text_chars);
                degraded = Some("link_text");
            }
        }
        let outcome = if let Some(how) = degraded {
            report.outcome = RenderingOutcome::Degraded;
            BlockRenderingOutcome::Degraded { how: how.into() }
        } else {
            BlockRenderingOutcome::Exact
        };
        report.blocks.push(BlockRendering {
            index: index as u16,
            outcome,
        });
    }
    if elements.is_empty() {
        add_text(&mut elements, "Choose an option", &mut text_chars);
    }
    if elements.len() > 100 || action > 100 || text_chars > 4000 {
        return Err(Error::Denied);
    }
    let card = json!({"schema":"2.0","config":{"update_multi":true},"body":{"elements":elements}});
    if serde_json::to_vec(&card).map_err(|_| Error::Denied)?.len() > 28 * 1024 {
        return Err(Error::Denied);
    }
    Ok((card, report))
}

fn add_text(elements: &mut Vec<Value>, text: &str, count: &mut usize) {
    if !text.is_empty() {
        *count += text.chars().count();
        elements.push(json!({"tag":"div","text":{"tag":"plain_text","content":text}}));
    }
}
fn link(url: &str, label: &str) -> Value {
    json!({"tag":"button","text":{"tag":"plain_text","content":label},
        "behaviors":[{"type":"open_url","default_url":url}]})
}

pub(super) fn markup(card: &Card) -> Result<Value, Error> {
    render(&card.message, &card.id, card.revision).map(|(markup, _)| markup)
}

impl InteractionStore {
    /// Lark does not normally include chat_type in card callbacks. Recover it
    /// only from a bound, current card before applying today's source ACL.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn lark_context(
        &self,
        channel: &ResolvedChannel,
        platform: Platform,
        route: &str,
        actor: &str,
        chat: &str,
        message: &str,
        data: &str,
    ) -> Result<ConversationKind, Error> {
        let (id, _, revision) = decode(data)?;
        let (card, _) = self.load(&id).await?;
        let lark = card.lark.as_ref().ok_or(Error::Denied)?;
        if !supported(platform)
            || lark.platform != platform
            || revision != Some(card.revision)
            || card.channel_id != channel.id
            || card.route_id != route
            || card.actor != actor
            || card.chat != chat
            || card.thread.is_some()
            || card.superseded
            || card.reference.is_none()
            || card
                .destination
                .as_ref()
                .is_none_or(|d| d.platform_id != message)
        {
            return Err(Error::Denied);
        }
        Ok(lark.conversation)
    }
}

#[cfg(test)]
#[path = "lark_tests.rs"]
mod tests;
