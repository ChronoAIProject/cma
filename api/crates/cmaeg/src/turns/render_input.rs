//! Neutral user input. Source envelopes and credentials never enter the prompt.
use cmaeg_core::{
    activity::{Activity, ActivityKind, SourceKind},
    channel::{ChannelDefinition, Platform, Source},
    profile::canonical_json,
};
use serde_json::{json, Value};
pub type ResponsesInputItem = Value;
pub fn render(activity: &Activity, definition: &ChannelDefinition) -> Vec<ResponsesInputItem> {
    let text = match &activity.kind {
        ActivityKind::Message {
            text,
            mentions_bot,
            reply_to,
            ..
        } => {
            let text = if *mentions_bot {
                strip_address(text, &activity.source, definition)
            } else {
                text.clone()
            };
            if reply_to.is_some() {
                format!("[replying to an earlier message]\n{text}")
            } else {
                text
            }
        }
        ActivityKind::Interaction(result) => {
            let label = result
                .values
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or(&result.action_id);
            format!("The user chose \"{label}\" (block {}).", result.block_id)
        }
        ActivityKind::Command { name, args } => format!("/{name} {args}").trim_end().to_owned(),
        ActivityKind::Event { ce } => {
            if let Some(template) = definition.sources.iter().find_map(|source| match source {
                Source::Custom {
                    prompt_template, ..
                } => prompt_template.as_deref(),
                _ => None,
            }) {
                return vec![
                    json!({"role":"user","content":[{"type":"input_text","text":crate::ingress::custom::template::render(template, ce)}]}),
                ];
            }
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
        }
    };
    vec![json!({"role":"user","content":[{"type":"input_text","text":text}]})]
}
fn strip_address(text: &str, source: &SourceKind, definition: &ChannelDefinition) -> String {
    if let SourceKind::NyxidRelay {
        platform: Platform::Lark | Platform::Feishu,
        ..
    } = source
    {
        return strip_mention_keys(text);
    }
    let Some(username) = definition.sources.iter().find_map(|source| match source {
        Source::NyxidRelay { bot_username, .. } => bot_username.as_deref(),
        _ => None,
    }) else {
        return text.to_owned();
    };
    let target = format!("@{username}");
    let mut output = String::new();
    let mut cursor = 0;
    for (index, _) in text.match_indices('@') {
        let end = index + target.len();
        if index < cursor
            || !text
                .get(index..end)
                .is_some_and(|part| part.eq_ignore_ascii_case(&target))
        {
            continue;
        }
        let boundary = |value: char| value.is_alphanumeric() || value == '_' || value == '@';
        if text[..index].chars().next_back().is_some_and(boundary)
            || text[end..].chars().next().is_some_and(boundary)
        {
            continue;
        }
        output.push_str(&text[cursor..index]);
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output.trim().to_owned()
}
/// Lark and Feishu replace each mention with a placeholder key such as `@_user_1`;
/// the keys that open the message address it and carry no meaning for the agent.
fn strip_mention_keys(text: &str) -> String {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix("@_user_") {
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let tail = &after[digits..];
        if digits == 0
            || tail
                .chars()
                .next()
                .is_some_and(|next| !next.is_whitespace())
        {
            break;
        }
        rest = tail.trim_start();
    }
    rest.trim_end().to_owned()
}
pub(super) fn bounded(value: &str, max: usize) -> &str {
    let mut end = value.len().min(max);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
