//! Only resource keys explicitly present in the authenticated inbound content.
use cmaeg_core::{
    activity::{Attachment, AttachmentKind, AttachmentSource, MAX_ATTACHMENTS},
    channel::LarkResourceKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resource {
    kind: AttachmentKind,
    key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thumbnail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
}

fn key(value: &Value) -> Result<String, ()> {
    value
        .as_str()
        .filter(|s| cmaeg_core::channel::lark_resources::safe_id(s, 1024))
        .map(str::to_owned)
        .ok_or(())
}

fn resource(kind: AttachmentKind, value: &Value) -> Result<Resource, ()> {
    let name = if kind == AttachmentKind::Image {
        "image_key"
    } else {
        "file_key"
    };
    let thumbnail = if kind == AttachmentKind::Video {
        value
            .get("image_key")
            .filter(|v| v.as_str() != Some(""))
            .map(key)
            .transpose()?
    } else {
        None
    };
    let filename = value
        .get("file_name")
        .map(|v| {
            v.as_str()
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 255
                        && !s.chars().any(cmaeg_core::activity::forbidden_text_char)
                })
                .map(str::to_owned)
                .ok_or(())
        })
        .transpose()?;
    let duration_ms = value
        .get("duration")
        .map(|v| v.as_u64().filter(|n| *n <= 86_400_000).ok_or(()))
        .transpose()?;
    Ok(Resource {
        kind,
        key: key(&value[name])?,
        thumbnail,
        filename,
        duration_ms,
    })
}

/// Preserve the inbound flat post, never search arbitrary nested objects for keys.
/// In particular content_v2 markdown, cards and forwarded submessages grant no keys.
pub(crate) fn parse(kind: &str, value: &Value) -> Result<Option<(String, Vec<Resource>)>, ()> {
    let mut resources = Vec::new();
    let text = match kind {
        "text" => value["text"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(())?
            .to_owned(),
        "image" | "file" | "audio" | "media" => {
            let kind = match kind {
                "image" => AttachmentKind::Image,
                "file" => AttachmentKind::File,
                "audio" => AttachmentKind::Audio,
                _ => AttachmentKind::Video,
            };
            resources.push(resource(kind, value)?);
            String::new()
        }
        "post" => {
            let mut text = value["title"].as_str().ok_or(())?.to_owned();
            let rows = value["content"]
                .as_array()
                .filter(|r| r.len() <= 256)
                .ok_or(())?;
            let mut elements = 0;
            for row in rows {
                text.push('\n');
                for item in row.as_array().ok_or(())? {
                    elements += 1;
                    if elements > 1024 {
                        return Err(());
                    }
                    match item["tag"].as_str().ok_or(())? {
                        "text" | "code_block" => text.push_str(item["text"].as_str().ok_or(())?),
                        "a" => {
                            text.push_str(item["text"].as_str().ok_or(())?);
                            text.push_str(" (");
                            text.push_str(item["href"].as_str().ok_or(())?);
                            text.push(')');
                        }
                        "at" => text.push_str(item["user_id"].as_str().ok_or(())?),
                        "emotion" => {
                            text.push(':');
                            text.push_str(&key(&item["emoji_type"])?);
                            text.push(':');
                        }
                        "hr" => text.push_str("\n---\n"),
                        "img" => resources.push(resource(AttachmentKind::Image, item)?),
                        "media" => resources.push(resource(AttachmentKind::Video, item)?),
                        // Unknown tags may hide content or keys: refuse the whole post.
                        _ => return Err(()),
                    }
                    if resources.len() > MAX_ATTACHMENTS || text.len() > 32 * 1024 {
                        return Err(());
                    }
                }
            }
            text
        }
        _ => return Ok(None),
    };
    if text.len() > 32 * 1024 || (text.is_empty() && resources.is_empty()) {
        return Err(());
    }
    Ok(Some((text, resources)))
}

pub(crate) fn attachments(
    resources: &[Resource],
    message: &str,
    revision: &str,
) -> Vec<Attachment> {
    resources
        .iter()
        .map(|r| {
            let locator = |key: &str, resource_kind| AttachmentSource::NativeLark {
                message_id: message.into(),
                resource_key: key.into(),
                resource_kind,
                source_revision: revision.into(),
            };
            Attachment {
                reference: format!("native-lark:{}:{}", message, r.key),
                kind: r.kind,
                filename: r.filename.clone(),
                mime_type: None,
                size_bytes: None,
                source: Some(locator(
                    &r.key,
                    if r.kind == AttachmentKind::Image {
                        LarkResourceKind::Image
                    } else {
                        LarkResourceKind::File
                    },
                )),
                thumbnail: r
                    .thumbnail
                    .as_ref()
                    .map(|k| locator(k, LarkResourceKind::Image)),
                duration_seconds: r.duration_ms.map(|ms| ms.div_ceil(1000) as u32),
                variant: None,
                width: None,
                height: None,
                emoji: None,
            }
        })
        .collect()
}
