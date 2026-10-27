//! Ordinary single-media edits: exact message IDs, one request, verified Message response.
use super::{media::message_identity, Error, TelegramClient};
use crate::{
    media::outbound::PreparedFile,
    native_media::model::{FileFact, Snapshot},
};
use cmaeg_core::{channel::Secret, message::NativeMediaKind};
use http_body_util::BodyExt;
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE},
    multipart::{Form, Part},
};
use serde_json::{json, Value};

pub(crate) struct PreparedEdit {
    method: &'static str,
    headers: HeaderMap,
    body: Vec<u8>,
}
impl TelegramClient {
    pub(crate) async fn prepare_media_edit(
        &self,
        chat: &str,
        message: &str,
        kind: NativeMediaKind,
        caption: &str,
        file: Option<PreparedFile>,
    ) -> Result<PreparedEdit, Error> {
        if caption.encode_utf16().count() > 1024 {
            return Err(Error::Rejected);
        }
        let Some(file) = file else {
            return Ok(PreparedEdit {
                method: "editMessageCaption",
                headers: HeaderMap::from_iter([(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )]),
                body: serde_json::to_vec(
                    &json!({"chat_id":chat,"message_id":message,"caption":caption}),
                )
                .map_err(|_| Error::Rejected)?,
            });
        };
        let field = family(kind).ok_or(Error::Rejected)?;
        let media = json!({"type":field,"media":"attach://replacement","caption":caption});
        let part = Part::bytes(file.bytes)
            .file_name(file.filename)
            .mime_str(&file.mime)
            .map_err(|_| Error::Rejected)?;
        let form = Form::new()
            .text("chat_id", chat.to_owned())
            .text("message_id", message.to_owned())
            .text("media", media.to_string())
            .part("replacement", part);
        let mut request = self
            .http
            .post(self.origin.clone())
            .multipart(form)
            .build()
            .map_err(|_| Error::Rejected)?;
        let body = request
            .body_mut()
            .take()
            .ok_or(Error::Rejected)?
            .collect()
            .await
            .map_err(|_| Error::Rejected)?
            .to_bytes()
            .to_vec();
        if body.len() > crate::media::outbound::MAX_OUTBOUND_BYTES as usize + 16384 {
            return Err(Error::Rejected);
        }
        Ok(PreparedEdit {
            method: "editMessageMedia",
            headers: request.headers().clone(),
            body,
        })
    }
    pub(crate) async fn edit_media(
        &self,
        token: &Secret,
        edit: PreparedEdit,
    ) -> Result<Value, Error> {
        let endpoint = self
            .origin
            .join(&format!("/bot{}/{}", token.as_str(), edit.method))
            .map_err(|_| Error::Rejected)?;
        let reply = self
            .transport
            .post(endpoint, edit.headers, edit.body, self.timeout)
            .await
            .map_err(|_| Error::Unconfirmed)?;
        let bytes = reply.body.map_err(|_| Error::Unconfirmed)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unconfirmed)?;
        if reply.status.as_u16() == 429 || value["error_code"] == 429 {
            return Err(Error::RateLimited(
                value["parameters"]["retry_after"]
                    .as_u64()
                    .unwrap_or(1)
                    .clamp(1, 3600),
            ));
        }
        if !reply.status.is_success() || value["ok"] != true {
            return Err(
                if reply.status.is_client_error() && reply.status.as_u16() != 408 {
                    Error::Rejected
                } else {
                    Error::Unconfirmed
                },
            );
        }
        Ok(value["result"].clone())
    }
}
pub(crate) fn family(kind: NativeMediaKind) -> Option<&'static str> {
    match kind {
        NativeMediaKind::Image => Some("photo"),
        NativeMediaKind::File => Some("document"),
        NativeMediaKind::Audio => Some("audio"),
        NativeMediaKind::Video => Some("video"),
        _ => None,
    }
}
pub(crate) fn snapshot(
    value: &Value,
    bot: &str,
    chat: &str,
    thread: Option<&str>,
    message: &str,
    kind: NativeMediaKind,
) -> Result<Snapshot, Error> {
    let bad = || Error::Unconfirmed;
    if message_identity(value, chat, thread)? != message
        || value["from"]["id"]
            .as_i64()
            .map(|id| id.to_string())
            .as_deref()
            != Some(bot)
        || value["from"]["is_bot"] != true
        || value.get("reply_markup").is_some()
        || value.get("business_connection_id").is_some()
        || value.get("sender_business_bot").is_some()
        || value
            .get("caption_entities")
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(bad());
    }
    let wanted = family(kind).ok_or_else(bad)?;
    for field in [
        "photo",
        "document",
        "audio",
        "video",
        "animation",
        "voice",
        "sticker",
        "paid_media",
        "live_photo",
        "text",
    ] {
        if field != wanted && value.get(field).is_some() {
            return Err(bad());
        }
    }
    let caption = match value.get("caption") {
        None => "",
        Some(v) => v.as_str().ok_or_else(bad)?,
    };
    if caption.len() > 4096 || caption.encode_utf16().count() > 1024 {
        return Err(bad());
    }
    let items: Vec<&Value> = if wanted == "photo" {
        let photos = value[wanted]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 24)
            .ok_or_else(bad)?;
        photos.iter().collect()
    } else {
        vec![&value[wanted]]
    };
    let mut files = Vec::new();
    for item in items {
        let id = opaque(&item["file_unique_id"])?;
        if files.iter().any(|f: &FileFact| f.id == id) {
            return Err(bad());
        }
        let size = number(item, "file_size")?;
        let width = number(item, "width")?;
        let height = number(item, "height")?;
        if wanted == "photo" && (width.is_none() || height.is_none()) {
            return Err(bad());
        }
        files.push(FileFact {
            id,
            locator: Some(opaque(&item["file_id"])?),
            filename: optional_text(item, "file_name", 255)?,
            mime: optional_text(item, "mime_type", 255)?,
            size,
            width,
            height,
        });
    }
    files.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Snapshot {
        kind,
        files,
        caption: caption.into(),
        content: String::new(),
    })
}
fn opaque(v: &Value) -> Result<String, Error> {
    v.as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 512
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .map(str::to_owned)
        .ok_or(Error::Unconfirmed)
}
fn optional_text(v: &Value, key: &str, max: usize) -> Result<Option<String>, Error> {
    v.get(key)
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or(Error::Unconfirmed)
        })
        .transpose()
}
fn number(v: &Value, key: &str) -> Result<Option<u64>, Error> {
    v.get(key)
        .map(|v| v.as_u64().filter(|n| *n > 0).ok_or(Error::Unconfirmed))
        .transpose()
}
