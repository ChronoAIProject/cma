//! Native uploads use bounded prepared bytes, never platform file locators or remote URLs.
use super::{positive, valid_token, Error, TelegramClient};
use crate::media::outbound::{PreparedFile, MAX_OUTBOUND_BYTES};
use cmaeg_core::{channel::Secret, message::NativeMediaKind};
use http_body_util::BodyExt;
use reqwest::{
    header::HeaderMap,
    multipart::{Form, Part},
};
use serde_json::{json, Value};

/// Fully encoded before the first platform write; no credentials or editable reference.
pub(crate) struct MediaUpload {
    method: &'static str,
    chat: String,
    thread: Option<String>,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl TelegramClient {
    pub(crate) async fn prepare_media(
        &self,
        chat: &str,
        thread: Option<&str>,
        kind: NativeMediaKind,
        file: PreparedFile,
    ) -> Result<MediaUpload, Error> {
        if !chat
            .parse::<i64>()
            .is_ok_and(|id| id != 0 && id.to_string() == chat)
            || thread.is_some_and(|id| {
                !id.parse::<i64>()
                    .is_ok_and(|n| n > 0 && n.to_string() == id)
            })
            || file.bytes.is_empty()
            || file.bytes.len() as u64 > MAX_OUTBOUND_BYTES
        {
            return Err(Error::Rejected);
        }
        let (method, field) = match kind {
            NativeMediaKind::Image => ("sendPhoto", "photo"),
            NativeMediaKind::File => ("sendDocument", "document"),
            NativeMediaKind::Audio => ("sendAudio", "audio"),
            NativeMediaKind::Video => ("sendVideo", "video"),
            NativeMediaKind::Sticker => return Err(Error::Rejected),
        };
        let part = Part::bytes(file.bytes)
            .file_name(file.filename)
            .mime_str(&file.mime)
            .map_err(|_| Error::Rejected)?;
        let mut form = Form::new()
            .text("chat_id", chat.to_owned())
            .part(field, part);
        if let Some(thread) = thread {
            form = form.text("message_thread_id", thread.to_owned());
        }
        if let Some(caption) = file.caption {
            form = form.text("caption", caption);
        }
        // The reqwest encoder escapes multipart metadata; collection is safe because
        // every part is an already bounded in-memory byte vector or validated string.
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
        if body.len() as u64 > MAX_OUTBOUND_BYTES + 16 * 1024 {
            return Err(Error::Rejected);
        }
        Ok(MediaUpload {
            method,
            chat: chat.to_owned(),
            thread: thread.map(str::to_owned),
            headers: request.headers().clone(),
            body,
        })
    }

    /// Every native upload is a single attempt. An unconfirmed write is never retried.
    pub(crate) async fn send_media_result(
        &self,
        token: &Secret,
        bot: &str,
        upload: MediaUpload,
    ) -> Result<Value, Error> {
        if !media_token_matches(token, bot) {
            return Err(Error::Rejected);
        }
        let endpoint = self
            .origin
            .join(&format!("/bot{}/{}", token.as_str(), upload.method))
            .map_err(|_| Error::Rejected)?;
        let reply = self
            .transport
            .post(endpoint, upload.headers, upload.body, self.timeout)
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
            return Err(if reply.status.is_client_error() {
                Error::Rejected
            } else {
                Error::Unconfirmed
            });
        }
        message_identity(&value["result"], &upload.chat, upload.thread.as_deref())?;
        Ok(value["result"].clone())
    }

    pub(crate) async fn send_media(
        &self,
        token: &Secret,
        bot: &str,
        upload: MediaUpload,
    ) -> Result<String, Error> {
        let value = self.send_media_result(token, bot, upload).await?;
        positive(&value["message_id"])
    }

    /// Text in a media batch uses the same sealed topic and stricter receipt check.
    /// The retained ordinary text/effect path is independent of this new path.
    pub(crate) async fn send_media_text(
        &self,
        token: &Secret,
        bot: &str,
        chat: &str,
        thread: Option<&str>,
        text: &str,
    ) -> Result<String, Error> {
        if !media_token_matches(token, bot) {
            return Err(Error::Rejected);
        }
        let mut body = json!({"chat_id": chat, "text": text});
        if let Some(thread) = thread {
            body["message_thread_id"] = thread.into();
        }
        let value = self.call(token, "sendMessage", &body).await?;
        message_identity(&value, chat, thread)
    }
}

pub(crate) fn media_token_matches(token: &Secret, bot: &str) -> bool {
    valid_token(token.as_str())
        && token
            .as_str()
            .split_once(':')
            .is_some_and(|(id, _)| id == bot)
}
pub(super) fn message_identity(
    value: &Value,
    chat: &str,
    thread: Option<&str>,
) -> Result<String, Error> {
    let actual_chat = value["chat"]["id"].as_i64().map(|id| id.to_string());
    let actual_thread = value.get("message_thread_id").map(positive).transpose()?;
    // These individual send methods cannot produce albums. Telegram redirects
    // album reactions to the first surviving message, so never accept that scope.
    if actual_chat.as_deref() != Some(chat)
        || actual_thread.as_deref() != thread
        || value.get("media_group_id").is_some()
    {
        return Err(Error::Unconfirmed);
    }
    positive(&value["message_id"])
}
