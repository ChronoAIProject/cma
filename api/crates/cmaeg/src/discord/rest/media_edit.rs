//! API v10 final attachment membership, exact pre-read and single PATCH confirmation.
use super::{valid_id, DiscordRest, RestError};
use crate::{
    media::outbound::PreparedFile,
    native_media::model::{FileFact, Observation, Snapshot},
};
use cmaeg_core::{
    activity::ActivityConversation,
    channel::Secret,
    message::{NativeMediaKind, NativeMediaUpdate},
};
use http_body_util::BodyExt;
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE},
    multipart::{Form, Part},
};
use serde_json::{json, Value};

pub(crate) struct PreparedEdit {
    headers: HeaderMap,
    body: Vec<u8>,
}
impl DiscordRest {
    pub(crate) async fn verify_media_bot(
        &self,
        token: &Secret,
        bot: &str,
    ) -> Result<(), RestError> {
        if !valid_id(bot) {
            return Err(RestError::Rejected);
        }
        self.ready(token)?;
        let response = self
            .client
            .get(self.url("users/@me")?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        let body = self.decode(response, token, false).await?;
        if body["id"].as_str() != Some(bot) || body["bot"] != true {
            return Err(RestError::Rejected);
        }
        Ok(())
    }
    pub(crate) async fn media_channel(
        &self,
        token: &Secret,
        channel: &str,
        conversation: &ActivityConversation,
    ) -> Result<Value, RestError> {
        self.ready(token)?;
        let response = self
            .client
            .get(self.url(&format!("channels/{channel}"))?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        let body = self.decode(response, token, false).await?;
        channel_fact(&body, channel, conversation)
    }
    pub(crate) async fn prepare_media_edit(
        &self,
        patch: &NativeMediaUpdate,
        current: &Snapshot,
        file: Option<PreparedFile>,
    ) -> Result<PreparedEdit, RestError> {
        let spoiler = super::media_presentation::spoiler(current.presentation.as_ref())?;
        if file
            .as_ref()
            .is_some_and(|f| f.filename.starts_with("SPOILER_") && !spoiler)
        {
            return Err(RestError::Rejected);
        }
        let payload = payload(patch, current, file.as_ref())?;
        self.prepare_media_payload(payload, file).await
    }
    pub(super) async fn prepare_media_payload(
        &self,
        payload: Value,
        file: Option<PreparedFile>,
    ) -> Result<PreparedEdit, RestError> {
        self.prepare_media_payload_set(payload, file.into_iter().collect())
            .await
    }
    pub(super) async fn prepare_media_payload_set(
        &self,
        payload: Value,
        files: Vec<PreparedFile>,
    ) -> Result<PreparedEdit, RestError> {
        if files.is_empty() {
            return Ok(PreparedEdit {
                headers: HeaderMap::from_iter([(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )]),
                body: serde_json::to_vec(&payload).map_err(|_| RestError::Rejected)?,
            });
        }
        if files.len() > cmaeg_core::message::MAX_NATIVE_ATTACHMENTS {
            return Err(RestError::Rejected);
        }
        let mut form = Form::new().text("payload_json", payload.to_string());
        for (index, file) in files.into_iter().enumerate() {
            let part = Part::bytes(file.bytes)
                .file_name(file.filename)
                .mime_str(&file.mime)
                .map_err(|_| RestError::Rejected)?;
            form = form.part(format!("files[{index}]"), part);
        }
        let mut request = self
            .client
            .patch(self.origin.clone())
            .multipart(form)
            .build()
            .map_err(|_| RestError::Rejected)?;
        let body = request
            .body_mut()
            .take()
            .ok_or(RestError::Rejected)?
            .collect()
            .await
            .map_err(|_| RestError::Rejected)?
            .to_bytes()
            .to_vec();
        if body.len() > crate::media::outbound::MAX_OUTBOUND_BYTES as usize + 16384 {
            return Err(RestError::Rejected);
        }
        Ok(PreparedEdit {
            headers: request.headers().clone(),
            body,
        })
    }
    pub(crate) async fn edit_media(
        &self,
        token: &Secret,
        channel: &str,
        message: &str,
        edit: PreparedEdit,
    ) -> Result<Value, RestError> {
        self.ready(token)?;
        let response = self
            .client
            .patch(self.url(&format!("channels/{channel}/messages/{message}"))?)
            .headers(edit.headers)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .body(edit.body)
            .send()
            .await
            .map_err(|_| RestError::Unknown)?;
        self.decode(response, token, true).await
    }
}
pub(crate) fn payload(
    patch: &NativeMediaUpdate,
    current: &Snapshot,
    file: Option<&PreparedFile>,
) -> Result<Value, RestError> {
    let mut payload = json!({});
    if let Some(text) = &patch.text {
        payload["content"] = text.as_str().into();
        payload["allowed_mentions"] = json!({"parse":[],"replied_user":false});
    }
    let caption = patch.caption.as_deref().unwrap_or(&current.caption);
    let spoiler = super::media_presentation::spoiler(current.presentation.as_ref())?;
    if let Some(file) = file {
        payload["attachments"] =
            json!([{"id":0,"filename":file.filename,"description":caption,"is_spoiler":spoiler}]);
    } else if patch.caption.is_some() {
        let file = current
            .files
            .first()
            .filter(|_| current.files.len() == 1)
            .ok_or(RestError::Rejected)?;
        payload["attachments"] = json!([{"id":file.id,"description":caption,"is_spoiler":spoiler}]);
    }
    Ok(payload)
}
pub(crate) fn observe(
    value: &Value,
    bot: &str,
    channel: &str,
    message: &str,
    kind: NativeMediaKind,
) -> Result<Observation, RestError> {
    let bad = || RestError::Unknown;
    if value["id"].as_str() != Some(message)
        || !valid_id(message)
        || value["channel_id"].as_str() != Some(channel)
        || value["author"]["id"].as_str() != Some(bot)
        || value["author"]["bot"] != true
        || !matches!(value["type"].as_u64(), Some(0 | 19))
    {
        return Err(bad());
    }
    for key in [
        "webhook_id",
        "interaction",
        "interaction_metadata",
        "message_snapshots",
        "poll",
        "call",
        "activity",
        "application",
    ] {
        if value.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad());
        }
    }
    for key in ["components", "sticker_items"] {
        if value
            .get(key)
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
        {
            return Err(bad());
        }
    }
    let content = value["content"]
        .as_str()
        .filter(|s| s.len() <= 8000 && s.encode_utf16().count() <= 2000)
        .ok_or_else(bad)?;
    let files = value["attachments"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(bad)?;
    let f = &files[0];
    let id = f["id"].as_str().filter(|id| valid_id(id)).ok_or_else(bad)?;
    let filename = f["filename"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 255
                && !s.contains(['/', '\\'])
                && !s.chars().any(char::is_control)
        })
        .ok_or_else(bad)?;
    if f.get("ephemeral").is_some_and(|v| v != false)
        || f.get("waveform").is_some()
        || ["clip_participants", "clip_created_at", "application"]
            .iter()
            .any(|key| f.get(*key).is_some())
    {
        return Err(bad());
    }
    let caption = match f.get("description") {
        None | Some(Value::Null) => "",
        Some(v) => v.as_str().ok_or_else(bad)?,
    };
    if caption.len() > 4096 || caption.encode_utf16().count() > 1024 {
        return Err(bad());
    }
    let mime = f
        .get("content_type")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= 255 && !s.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or_else(bad)
        })
        .transpose()?;
    let number = |key| {
        f.get(key)
            .filter(|v| !v.is_null())
            .map(|v| v.as_u64().filter(|n| *n > 0).ok_or_else(bad))
            .transpose()
    };
    let size = number("size")?.ok_or_else(bad)?;
    if size > crate::media::outbound::MAX_OUTBOUND_BYTES {
        return Err(bad());
    }
    let (presentation, eligible) = super::media_presentation::observe(value, f)?;
    Ok(Observation {
        snapshot: Snapshot {
            kind,
            files: vec![FileFact {
                id: id.into(),
                locator: None,
                filename: Some(filename.into()),
                mime,
                size: Some(size),
                width: number("width")?,
                height: number("height")?,
                metadata: super::media_presentation::asset_metadata(f)?,
            }],
            caption: caption.into(),
            content: content.into(),
            presentation: Some(presentation),
        },
        eligible,
    })
}
fn channel_fact(
    body: &Value,
    channel: &str,
    conversation: &ActivityConversation,
) -> Result<Value, RestError> {
    let bad = || RestError::Rejected;
    if body["id"].as_str() != Some(channel) || conversation.id != channel {
        return Err(bad());
    }
    let kind = body["type"].as_u64().ok_or_else(bad)?;
    if conversation.thread_id.is_some() {
        let thread =
            crate::discord::component_thread::ThreadIdentity::from_channel(body).ok_or_else(bad)?;
        if !thread.matches(conversation)
            || body["thread_metadata"]["archived"] != false
            || body["thread_metadata"]["locked"] != false
        {
            return Err(bad());
        }
    } else {
        let ordinary = match conversation.kind {
            cmaeg_core::activity::ConversationKind::Private => {
                kind == 1 && body["guild_id"].is_null()
            }
            cmaeg_core::activity::ConversationKind::Group => {
                matches!(kind, 0 | 2 | 5 | 13) && body["guild_id"].as_str().is_some_and(valid_id)
            }
            _ => false,
        };
        if !ordinary {
            return Err(bad());
        }
    }
    Ok(json!({"id":channel,"type":kind,"guild_id":body["guild_id"],"parent_id":body["parent_id"]}))
}

pub(crate) fn snapshot(
    value: &Value,
    bot: &str,
    channel: &str,
    message: &str,
    kind: NativeMediaKind,
) -> Result<Snapshot, RestError> {
    let observed = observe(value, bot, channel, message, kind)?;
    if observed.eligible {
        Ok(observed.snapshot)
    } else {
        Err(RestError::Unknown)
    }
}
