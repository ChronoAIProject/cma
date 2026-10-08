//! Neutral user input. Source envelopes and credentials never enter the prompt.
use super::render_media::{render_block, Audience, RenderMedia};
use crate::ingress::custom::attachments::elided;
use cmaeg_core::{
    activity::{Activity, ActivityKind, Attachment, AttachmentSource},
    channel::{ChannelDefinition, Source},
    ids::ConsumerId,
    profile::canonical_json,
};
use serde_json::{json, Value};
pub type ResponsesInputItem = Value;
/// Render one user message with a single `input_text` part. An activity with
/// attachments gains the `[Attachments]` block. Routed activities strip
/// resolved addressing and identify the serving target; legacy input is unchanged.
pub fn render(
    activity: &Activity,
    definition: &ChannelDefinition,
    media: &RenderMedia,
    audience: Audience,
    consumer: Option<&ConsumerId>,
) -> Vec<ResponsesInputItem> {
    let text = match &activity.kind {
        ActivityKind::Message {
            text,
            attachments,
            mentions_bot,
            reply_to,
            album_id,
        } => {
            // Routing validation pins an in-range UTF-8 boundary into this
            // activity. Legacy text has no routing offset.
            let text = addressed_remainder(text, activity);
            let text = if *mentions_bot {
                cmaeg_core::chat::strip_address(
                    text,
                    &activity.source,
                    addressed_username(definition, activity),
                )
            } else {
                text.to_owned()
            };
            let text = if reply_to.is_some() {
                format!("[replying to an earlier message]\n{text}")
            } else {
                text
            };
            with_block(
                text,
                render_block(attachments, album_id.as_deref(), media, audience),
            )
        }
        ActivityKind::Interaction(result) => {
            let label = result
                .values
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or(&result.action_id);
            format!("The user chose \"{label}\" (block {}).", result.block_id)
        }
        ActivityKind::Command { name, args }
            if name == "to"
                && activity.routing.as_ref().is_some_and(|routing| {
                    routing.resolution == cmaeg_core::activity::RouteResolution::Command
                }) =>
        {
            addressed_remainder(args, activity).to_owned()
        }
        ActivityKind::Command { name, args } => format!("/{name} {args}").trim_end().to_owned(),
        ActivityKind::Event { ce, attachments } => {
            // Inline payloads are base64 and item URLs are locators; neither
            // may reach the prompt, which lists the items in its block instead.
            let ce = match elision_pointer(attachments, definition) {
                Some(pointer) => elided(ce, &pointer, attachments.len()),
                None => std::borrow::Cow::Borrowed(ce),
            };
            let text = if let Some(template) =
                definition.sources.iter().find_map(|source| match source {
                    Source::Custom {
                        prompt_template, ..
                    } => prompt_template.as_deref(),
                    _ => None,
                }) {
                crate::ingress::custom::template::render(template, &ce)
            } else {
                let data = ce
                    .data
                    .as_ref()
                    .map(|data| canonical_json(data.clone()))
                    .unwrap_or_else(|| "null".into());
                format!(
                    "Event {} from {}:\n{}",
                    ce.r#type,
                    ce.source,
                    bounded(&data, 8192)
                )
            };
            with_block(text, render_block(attachments, None, media, audience))
        }
    };
    let text = match &activity.routing {
        Some(routing) if routing.targets.len() > 1 => {
            let default = ConsumerId::default_consumer();
            let serving = consumer.unwrap_or(&default);
            let name = routing
                .targets
                .iter()
                .find(|target| &target.id == serving)
                .map_or("unknown", |target| target.name.as_str());
            let names = routing
                .targets
                .iter()
                .map(|target| format!("@{}", target.name))
                .collect::<Vec<_>>()
                .join(" and ");
            format!("[Routed to {names}; you are @{name}]\n{text}")
        }
        _ => text,
    };
    vec![json!({"role":"user","content":[{"type":"input_text","text":text}]})]
}
fn addressed_remainder<'a>(text: &'a str, activity: &Activity) -> &'a str {
    let offset = activity
        .routing
        .as_ref()
        .map_or(0, |routing| usize::from(routing.addressing_bytes));
    &text[offset..]
}
/// Where an event with attachments listed them: the channel's configured
/// pointer. An inline locator records the pointer the event was admitted
/// under, which wins, so a setting changed or removed between admission and
/// rendering cannot expose that event's inline payloads.
fn elision_pointer(attachments: &[Attachment], definition: &ChannelDefinition) -> Option<String> {
    if attachments.is_empty() {
        return None;
    }
    let admitted = attachments.iter().enumerate().find_map(|(index, item)| {
        let Some(AttachmentSource::Inline { pointer }) = &item.source else {
            return None;
        };
        pointer
            .strip_suffix(&format!("/{index}/url"))
            .map(str::to_owned)
    });
    admitted.or_else(|| {
        definition.sources.iter().find_map(|source| match source {
            Source::Custom {
                attachments: Some(config),
                ..
            } => Some(config.pointer.clone()),
            _ => None,
        })
    })
}
/// A media-only message has blank text; its block alone keeps the input
/// non-empty, which the provider requires.
fn with_block(text: String, block: Option<String>) -> String {
    match block {
        None => text,
        Some(block) if text.trim().is_empty() => block,
        Some(block) => format!("{}\n\n{block}", text.trim_end()),
    }
}
/// The bot username a chat address names. On routed activities or extended
/// sources, that is the bot of the source the activity arrived on; a legacy
/// definition keeps the renderer's original choice, so its provider input
/// stays byte-identical (invariant I5).
fn addressed_username<'a>(
    definition: &'a ChannelDefinition,
    activity: &Activity,
) -> Option<&'a str> {
    if activity.routing.is_some() || definition.has_extended_sources() {
        definition.bot_username_of(&activity.source)
    } else {
        first_bot_username(definition)
    }
}
/// The first relay or Telegram source that declares a bot username, as the
/// renderer has always chosen it for legacy definitions.
fn first_bot_username(definition: &ChannelDefinition) -> Option<&str> {
    definition.sources.iter().find_map(|source| match source {
        Source::NyxidRelay { bot_username, .. } | Source::Telegram { bot_username, .. } => {
            bot_username.as_deref()
        }
        _ => None,
    })
}
pub(super) fn bounded(value: &str, max: usize) -> &str {
    let mut end = value.len().min(max);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
