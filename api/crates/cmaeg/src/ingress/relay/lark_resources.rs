//! Turn authenticated rich-post placeholders into explicitly scoped locators.
use super::{facts::RelayFacts, payload::CallbackPayload};
use crate::{channels::resolver::ResolvedChannel, media::source::lark::revision};
use cmaeg_core::{
    activity::{AttachmentKind, AttachmentSource, MediaVariant},
    channel::{lark_resources::safe_id, LarkResourceKind, Platform, Source},
};
use serde_json::Value;

pub(super) fn bind(
    channel: &ResolvedChannel,
    platform: Platform,
    payload: &CallbackPayload,
    facts: &mut RelayFacts,
) {
    if !matches!(platform, Platform::Lark | Platform::Feishu) {
        return;
    }
    let Some(key) = channel.definition.secrets.channel_key.as_ref() else {
        return;
    };
    let Some(source @ Source::NyxidRelay { bot_id: Some(bot_id), lark_resources: Some(binding), .. }) = channel.definition.sources.iter().find(|source| {
        matches!(source, Source::NyxidRelay { platform: current, route_ids, .. } if *current == platform && route_ids.contains(&payload.conversation.id))
    }) else { return; };
    let Some(raw) = &payload.raw_platform_data else {
        return;
    };
    if raw.pointer("/header/app_id").and_then(Value::as_str) != Some(binding.app_id.as_str())
        || raw
            .pointer("/event/message/message_type")
            .and_then(Value::as_str)
            != Some("post")
    {
        return;
    }
    let Some(message_id) = raw
        .pointer("/event/message/message_id")
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("om_") && safe_id(id, 128))
    else {
        return;
    };
    let Some(source_revision) = revision(source, key) else {
        return;
    };
    for attachment in &mut facts.attachments {
        if attachment.source.is_some() || attachment.variant != Some(MediaVariant::Embedded) {
            continue;
        }
        let Some(resource_key) = attachment.reference.strip_prefix("lark:").filter(|key| {
            safe_id(key, 1024) && (key.starts_with("img_") || key.starts_with("file_"))
        }) else {
            continue;
        };
        let resource_kind = match attachment.kind {
            AttachmentKind::Image if resource_key.starts_with("img_") => LarkResourceKind::Image,
            AttachmentKind::Video if resource_key.starts_with("file_") => LarkResourceKind::File,
            _ => continue,
        };
        attachment.source = Some(AttachmentSource::LarkResource {
            message_id: message_id.to_owned(),
            resource_key: resource_key.to_owned(),
            resource_kind,
            binding: binding.clone(),
            bot_id: bot_id.clone(),
            source_revision: source_revision.clone(),
        });
    }
}
