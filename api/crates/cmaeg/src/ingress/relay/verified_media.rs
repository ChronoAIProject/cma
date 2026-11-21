//! Bind raw Discord/WhatsApp media to an authenticated relay download index.
//! A missing or inconsistent binding stays visible but grants no byte access.
use super::attachments::RelayAttachments;
use cmaeg_core::activity::{Attachment, AttachmentKind, AttachmentSource, MediaVariant};
use serde_json::Value;

pub(super) struct Identity<'a> {
    pub platform: &'static str,
    pub message_id: &'a str,
    pub file_id: Option<&'a str>,
    /// Discord's exact CDN handle; WhatsApp binds its numeric resource key.
    pub url: Option<&'a str>,
}

pub(super) fn extract(
    identity: Identity<'_>,
    raw: &Value,
    kind: AttachmentKind,
    envelope: &RelayAttachments,
    ordinal: usize,
) -> Attachment {
    let (label, variant) = match kind {
        AttachmentKind::Image => ("image", MediaVariant::Image),
        AttachmentKind::Audio => ("audio", MediaVariant::Audio),
        AttachmentKind::Video => ("video", MediaVariant::Video),
        _ => ("file", MediaVariant::File),
    };
    let item = identity.file_id.and_then(|key| {
        envelope.kept.iter().find(|item| {
            let item = &item.attachment;
            item.file_key.as_deref() == Some(key)
                && item.platform_message_id.as_deref() == Some(identity.message_id)
                && item.content_type == label
                && !item.url.is_empty()
                && identity.url.is_none_or(|url| item.url == url)
        })
    });
    let mut attachment = Attachment {
        reference: identity.file_id.map_or_else(
            || format!("{}:item-{ordinal}", identity.platform),
            |key| format!("{}:{key}", identity.platform),
        ),
        kind,
        filename: raw["filename"].as_str().map(str::to_owned),
        mime_type: raw["mime_type"]
            .as_str()
            .or_else(|| raw["content_type"].as_str())
            .map(str::to_owned),
        size_bytes: raw["size"].as_u64(),
        source: item.map(|item| AttachmentSource::NyxidRelay { index: item.index }),
        variant: Some(variant),
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    };
    let repairs = attachment.normalize_facts();
    for field in repairs.fields() {
        metrics::counter!(crate::telemetry::names::MEDIA_FACTS_REPAIRED,
            crate::telemetry::names::SOURCE => "nyxid_relay",
            crate::telemetry::names::FIELD => field)
        .increment(1);
    }
    metrics::counter!(crate::telemetry::names::MEDIA_RELAY_CORRELATION,
        crate::telemetry::names::OUTCOME => if item.is_some() { "matched" } else { "unmatched" })
    .increment(1);
    attachment
}

pub(super) fn numeric_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| {
        !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_digit())
    })
}
