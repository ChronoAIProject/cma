//! Native uploads from fully materialized bytes; keys cannot be supplied by callers.
#[path = "outbound/file.rs"]
mod file;
use super::{Client, Error};
use crate::media::{
    outbound::{PreparedFile, MAX_OUTBOUND_BYTES},
    outbound_profiles::{image::image_header, ogg, video},
    sniff::sniff,
};
use cmaeg_core::{
    channel::{lark_resources::safe_id, NativeLarkReply, Secret},
    message::{NativeAttachment, NativeMediaKind},
};
use http_body_util::BodyExt;
use reqwest::{
    header::HeaderMap,
    multipart::{Form, Part},
    Method,
};
use serde_json::json;

pub(crate) const MIME_TYPES: [&str; 14] = [
    "image/jpeg",
    "image/png",
    "application/pdf",
    "text/plain",
    "application/msword",
    "application/vnd.ms-excel",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/zip",
    "application/octet-stream",
    "audio/ogg",
    "video/mp4",
];
pub(crate) struct MediaUpload {
    headers: HeaderMap,
    body: Vec<u8>,
    kind: &'static str,
}
pub(crate) struct UploadedMedia {
    destination: NativeLarkReply,
    kind: &'static str,
    key: String,
}
impl UploadedMedia {
    /// Only a confirmed image uploaded for this exact app/chat/thread may enter a card.
    pub(crate) fn card_image_key(self, reply: &NativeLarkReply) -> Result<String, Error> {
        if self.destination != *reply || self.kind != "image" {
            return Err(Error::Invalid);
        }
        Ok(self.key)
    }
}
impl Client {
    /// All items are prepared before a token request or platform write. The provider
    /// validates payload decodability; this enforces the advertised structural subset.
    pub(crate) async fn prepare_media(
        item: &NativeAttachment,
        mut file: PreparedFile,
    ) -> Result<MediaUpload, Error> {
        if file.bytes.is_empty()
            || file.bytes.len() as u64 > MAX_OUTBOUND_BYTES
            || file.caption.is_some()
        {
            return Err(Error::Invalid);
        }
        let (kind, mime, file_type, extensions): (_, _, _, &[&str]) = match item.kind {
            NativeMediaKind::Image => {
                let mime = sniff(&file.bytes[..file.bytes.len().min(64)]).mime;
                if !matches!(mime, "image/jpeg" | "image/png") || !image_header(&file.bytes, mime) {
                    return Err(Error::Invalid);
                }
                (
                    "image",
                    mime,
                    "",
                    if mime == "image/png" {
                        &["png"]
                    } else {
                        &["jpg", "jpeg"]
                    },
                )
            }
            NativeMediaKind::File => {
                let profile = file::profile(item, &file.bytes)?;
                ("file", profile.mime, profile.file_type, profile.extensions)
            }
            NativeMediaKind::Audio if ogg::mono_opus(&file.bytes) => {
                ("audio", "audio/ogg", "opus", &["opus", "ogg"])
            }
            NativeMediaKind::Video if video::inspect(&file.bytes) == Some(video::Format::Mp4) => {
                ("media", "video/mp4", "mp4", &["mp4"])
            }
            _ => return Err(Error::Invalid),
        };
        if item
            .mime_type
            .as_deref()
            .is_some_and(|claimed| claimed != mime)
        {
            return Err(Error::Invalid);
        }
        if item.filename.is_none() {
            file.filename = format!(
                "{}.{}",
                file.filename,
                extensions.first().copied().unwrap_or("bin")
            );
        } else if !extensions.is_empty()
            && !file
                .filename
                .rsplit_once('.')
                .is_some_and(|(_, extension)| {
                    extensions.contains(&extension.to_ascii_lowercase().as_str())
                })
        {
            return Err(Error::Invalid);
        }
        if extensions.is_empty()
            && file
                .filename
                .rsplit_once('.')
                .is_none_or(|(_, ext)| ext.is_empty())
        {
            // Both regional upload contracts require a filename with a suffix.
            file.filename.push_str(".bin");
        }
        if file.filename.len() > 255 {
            return Err(Error::Invalid);
        }
        let part = Part::bytes(file.bytes)
            .file_name(file.filename.clone())
            .mime_str(mime)
            .map_err(|_| Error::Invalid)?;
        let form = if kind == "image" {
            Form::new()
                .text("image_type", "message")
                .part("image", part)
        } else {
            // Duration and cover are optional in both regional API contracts.
            Form::new()
                .text("file_type", file_type)
                .text("file_name", file.filename)
                .part("file", part)
        };
        // Build only: no network, credentials, or user-chosen upload destination.
        let mut request = reqwest::Client::new()
            .post("https://open.larksuite.com/")
            .multipart(form)
            .build()
            .map_err(|_| Error::Invalid)?;
        let body = request
            .body_mut()
            .take()
            .ok_or(Error::Invalid)?
            .collect()
            .await
            .map_err(|_| Error::Invalid)?
            .to_bytes()
            .to_vec();
        if body.len() as u64 > MAX_OUTBOUND_BYTES + 16 * 1024 {
            return Err(Error::Invalid);
        }
        Ok(MediaUpload {
            headers: request.headers().clone(),
            body,
            kind,
        })
    }
    pub(crate) async fn upload_media(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        upload: MediaUpload,
    ) -> Result<UploadedMedia, Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        let (path, field) = if upload.kind == "image" {
            ("/open-apis/im/v1/images", "image_key")
        } else {
            ("/open-apis/im/v1/files", "file_key")
        };
        let value = self
            .call_bytes(
                &reply.identity,
                Method::POST,
                path,
                Some(token),
                upload.headers,
                upload.body,
            )
            .await?;
        let key = value["data"][field]
            .as_str()
            .filter(|key| safe_id(key, 1024))
            .ok_or(Error::Unconfirmed)?;
        Ok(UploadedMedia {
            destination: reply.clone(),
            kind: upload.kind,
            key: key.into(),
        })
    }
    pub(crate) async fn send_media(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        media: UploadedMedia,
    ) -> Result<String, Error> {
        if media.destination != *reply {
            return Err(Error::Invalid);
        }
        let content = if media.kind == "image" {
            json!({"image_key":media.key})
        } else {
            json!({"file_key":media.key})
        };
        self.send_content(token, reply, media.kind, content).await
    }
}

#[cfg(test)]
pub(crate) mod tests;
