//! One bounded, ordered Bot API album; never an editable aggregate reference.
use super::{Error, MediaUpload, PreparedFile, TelegramClient, MAX_OUTBOUND_BYTES};
use cmaeg_core::{
    channel::Secret,
    message::{NativeMediaKind, MAX_NATIVE_ATTACHMENTS},
};
use http_body_util::BodyExt;
use reqwest::multipart::{Form, Part};
use serde_json::json;
use std::collections::HashSet;

pub(crate) struct AlbumUpload {
    upload: MediaUpload,
    kinds: Vec<NativeMediaKind>,
    captions: Vec<Option<String>>,
}

/// Telegram permits mixed photos/videos, only documents, or only audio files.
pub(crate) fn album_supported(kinds: &[NativeMediaKind]) -> bool {
    (2..=MAX_NATIVE_ATTACHMENTS).contains(&kinds.len())
        && (kinds
            .iter()
            .all(|kind| matches!(kind, NativeMediaKind::Image | NativeMediaKind::Video))
            || kinds.iter().all(|kind| *kind == NativeMediaKind::File)
            || kinds.iter().all(|kind| *kind == NativeMediaKind::Audio))
}

fn media_type(kind: NativeMediaKind) -> &'static str {
    match kind {
        NativeMediaKind::Image => "photo",
        NativeMediaKind::Video => "video",
        NativeMediaKind::File => "document",
        NativeMediaKind::Audio => "audio",
        NativeMediaKind::Sticker => "sticker",
    }
}

impl TelegramClient {
    pub(crate) async fn prepare_album(
        &self,
        chat: &str,
        thread: Option<&str>,
        kinds: Vec<NativeMediaKind>,
        files: Vec<PreparedFile>,
    ) -> Result<AlbumUpload, Error> {
        if !album_supported(&kinds)
            || kinds.len() != files.len()
            || !chat
                .parse::<i64>()
                .is_ok_and(|id| id != 0 && id.to_string() == chat)
            || thread.is_some_and(|id| {
                !id.parse::<i64>()
                    .is_ok_and(|n| n > 0 && n.to_string() == id)
            })
            || files.iter().any(|f| {
                f.bytes.is_empty()
                    || f.bytes.len() as u64 > MAX_OUTBOUND_BYTES
                    || f.caption.as_ref().is_some_and(|c| c.len() > 1024)
            })
            || files.iter().map(|f| f.bytes.len() as u64).sum::<u64>() > MAX_OUTBOUND_BYTES
        {
            return Err(Error::Rejected);
        }
        let mut form = Form::new().text("chat_id", chat.to_owned());
        if let Some(thread) = thread {
            form = form.text("message_thread_id", thread.to_owned());
        }
        let mut media = Vec::with_capacity(files.len());
        let captions = files.iter().map(|f| f.caption.clone()).collect();
        for (index, (kind, file)) in kinds.iter().zip(files).enumerate() {
            let name = format!("attachment_{index}");
            let mut item = json!({"type":media_type(*kind),"media":format!("attach://{name}")});
            if let Some(caption) = file.caption {
                item["caption"] = caption.into();
            }
            let part = Part::bytes(file.bytes)
                .file_name(file.filename)
                .mime_str(&file.mime)
                .map_err(|_| Error::Rejected)?;
            form = form.part(name, part);
            media.push(item);
        }
        form = form.text(
            "media",
            serde_json::to_string(&media).map_err(|_| Error::Rejected)?,
        );
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
        Ok(AlbumUpload {
            upload: MediaUpload {
                method: "sendMediaGroup",
                chat: chat.to_owned(),
                thread: thread.map(str::to_owned),
                headers: request.headers().clone(),
                body,
            },
            kinds,
            captions,
        })
    }

    /// One attempt only. All members must be confirmed before acknowledging delivery.
    pub(crate) async fn send_album(
        &self,
        token: &Secret,
        bot: &str,
        album: AlbumUpload,
    ) -> Result<String, Error> {
        let chat = album.upload.chat.clone();
        let thread = album.upload.thread.clone();
        let value = self.send_upload(token, bot, album.upload).await?;
        let messages = value.as_array().ok_or(Error::Unconfirmed)?;
        if messages.len() != album.kinds.len() {
            return Err(Error::Unconfirmed);
        }
        let mut group = None;
        let mut ids = HashSet::new();
        let mut first = None;
        for ((value, kind), caption) in messages.iter().zip(album.kinds).zip(album.captions) {
            let id = super::positive(&value["message_id"])?;
            let actual_thread = value
                .get("message_thread_id")
                .map(super::positive)
                .transpose()?;
            let actual_group = value["media_group_id"]
                .as_str()
                .filter(|v| !v.is_empty() && v.len() <= 128 && !v.chars().any(char::is_control))
                .ok_or(Error::Unconfirmed)?;
            let file = &value[media_type(kind)];
            let has_media = if kind == NativeMediaKind::Image {
                file.as_array().is_some_and(|photos| !photos.is_empty())
            } else {
                file.is_object()
            };
            if value["chat"]["id"]
                .as_i64()
                .map(|id| id.to_string())
                .as_deref()
                != Some(chat.as_str())
                || actual_thread != thread
                || value["from"]["id"]
                    .as_i64()
                    .map(|id| id.to_string())
                    .as_deref()
                    != Some(bot)
                || value["from"]["is_bot"] != true
                || group.is_some_and(|expected| expected != actual_group)
                || !ids.insert(id.clone())
                || !has_media
                || value["caption"].as_str().unwrap_or("") != caption.as_deref().unwrap_or("")
            {
                return Err(Error::Unconfirmed);
            }
            group = Some(actual_group);
            first.get_or_insert(id);
        }
        // The first ID is a delivery acknowledgement, never public effect authority.
        first.ok_or(Error::Unconfirmed)
    }
}
