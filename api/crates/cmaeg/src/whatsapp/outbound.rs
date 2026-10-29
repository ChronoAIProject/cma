//! Bounded native image, document, audio, sticker and video uploads.
#[path = "outbound_audio.rs"]
pub(crate) mod audio;
#[path = "outbound_document.rs"]
pub(crate) mod document;
#[path = "outbound_sticker.rs"]
pub(crate) mod sticker;
use super::{Client, Error};
use crate::media::outbound_profiles::bmff;
use crate::media::outbound_profiles::image::image_header;
pub(crate) use crate::media::outbound_profiles::video;
use crate::media::{outbound::PreparedFile, sniff::sniff};
use cmaeg_core::{
    channel::{
        whatsapp::{valid_message_id, valid_meta_id},
        WhatsappCredentials,
    },
    message::{NativeAttachment, NativeMediaKind},
};
use http_body_util::BodyExt;
use reqwest::{
    header::HeaderMap,
    multipart::{Form, Part},
    Method,
};
use serde_json::{json, Value};

/// Meta documents 5 MB; use decimal bytes conservatively.
pub(crate) const MAX_IMAGE_BYTES: usize = 5_000_000;
pub(crate) const MIME_TYPES: [&str; 18] = [
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
    "audio/aac",
    "audio/amr",
    "audio/mpeg",
    "audio/mp4",
    "audio/ogg",
    "image/webp",
    "video/mp4",
    "video/3gpp",
];

pub(crate) struct MediaUpload {
    headers: HeaderMap,
    body: Vec<u8>,
    kind: &'static str,
    metadata: Value,
}
pub(crate) struct UploadedMedia {
    phone: String,
    kind: &'static str,
    metadata: Value,
}

impl UploadedMedia {
    /// Only this phone's upload can become a button header. Header media has no caption.
    pub(super) fn into_interactive_header(self, phone: &str) -> Result<Value, Error> {
        if self.phone != phone
            || !matches!(self.kind, "image" | "document" | "video")
            || self.metadata.get("caption").is_some()
        {
            return Err(Error::Invalid);
        }
        let mut header = json!({"type": self.kind});
        header[self.kind] = self.metadata;
        Ok(header)
    }
}

impl Client {
    /// Prepare the whole batch before any platform effect. Shared preparation has
    /// already bounded URLs, metadata, inline bytes and aggregate downloaded bytes.
    pub(crate) async fn prepare_media(
        item: &NativeAttachment,
        mut file: PreparedFile,
    ) -> Result<MediaUpload, Error> {
        if item.kind == NativeMediaKind::Sticker
            && (file.caption.is_some() || sticker::inspect(&file.bytes).is_none())
        {
            return Err(Error::Invalid);
        }
        let audio = if matches!(item.kind, NativeMediaKind::Audio | NativeMediaKind::Voice) {
            if file.caption.is_some() {
                return Err(Error::Invalid);
            }
            let format = audio::inspect(&file.bytes).ok_or(Error::Invalid)?;
            if item.kind == NativeMediaKind::Voice && format != audio::Format::Ogg {
                return Err(Error::Invalid);
            }
            Some(format)
        } else {
            None
        };
        let video = if item.kind == NativeMediaKind::Video {
            Some(video::inspect(&file.bytes).ok_or(Error::Invalid)?)
        } else {
            None
        };
        let document = if item.kind == NativeMediaKind::File {
            Some(document::inspect(&file.bytes).ok_or(Error::Invalid)?)
        } else {
            None
        };
        let mime = if let Some(document) = document {
            document.mime()
        } else if let Some(video) = video {
            video.mime()
        } else if let Some(audio) = audio {
            audio.mime()
        } else {
            sniff(&file.bytes[..file.bytes.len().min(64)]).mime
        };
        if item
            .mime_type
            .as_deref()
            .is_some_and(|claimed| claimed != mime)
            || file.bytes.is_empty()
            || file.bytes.len() as u64 > crate::media::outbound::MAX_OUTBOUND_BYTES
            || file
                .caption
                .as_ref()
                .is_some_and(|caption| caption.chars().count() > 1024)
        {
            return Err(Error::Invalid);
        }
        let (kind, extensions) = match (item.kind, mime) {
            (NativeMediaKind::Image, "image/jpeg") => ("image", &["jpg", "jpeg"][..]),
            (NativeMediaKind::Image, "image/png") => ("image", &["png"][..]),
            (NativeMediaKind::File, _) => {
                ("document", document.ok_or(Error::Invalid)?.extensions())
            }
            (NativeMediaKind::Audio | NativeMediaKind::Voice, _) => {
                ("audio", audio.ok_or(Error::Invalid)?.extensions())
            }
            (NativeMediaKind::Video, _) => ("video", video.ok_or(Error::Invalid)?.extensions()),
            (NativeMediaKind::Sticker, "image/webp") => ("sticker", &["webp"][..]),
            _ => return Err(Error::Invalid),
        };
        if kind == "image"
            && (file.bytes.len() > MAX_IMAGE_BYTES || !image_header(&file.bytes, mime))
        {
            return Err(Error::Invalid);
        }
        if item.filename.is_none() {
            file.filename = format!("{}.{}", file.filename, extensions[0]);
        } else if !file
            .filename
            .rsplit_once('.')
            .is_some_and(|(_, extension)| {
                extensions.contains(&extension.to_ascii_lowercase().as_str())
            })
        {
            return Err(Error::Invalid);
        }
        let mut metadata = json!({});
        if item.kind == NativeMediaKind::Voice {
            metadata["voice"] = true.into();
        }
        if let Some(caption) = file.caption {
            metadata["caption"] = caption.into();
        }
        if kind == "document" {
            metadata["filename"] = file.filename.clone().into();
        }
        let part = Part::bytes(file.bytes)
            .file_name(file.filename)
            .mime_str(mime)
            .map_err(|_| Error::Invalid)?;
        let form = Form::new()
            .text("messaging_product", "whatsapp")
            .text("type", mime)
            .part("file", part);
        // Building and collecting reqwest's multipart body does no network IO.
        let mut request = reqwest::Client::new()
            .post(Self::endpoint("media")?)
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
        if body.len() as u64 > crate::media::outbound::MAX_OUTBOUND_BYTES + 16 * 1024 {
            return Err(Error::Invalid);
        }
        Ok(MediaUpload {
            headers: request.headers().clone(),
            body,
            kind,
            metadata,
        })
    }

    /// One authenticated write. An uncertain upload never proceeds to a send.
    pub(crate) async fn upload_media(
        &self,
        credentials: &WhatsappCredentials,
        phone: &str,
        upload: MediaUpload,
    ) -> Result<UploadedMedia, Error> {
        if !valid_meta_id(phone) || credentials.validate().is_err() {
            return Err(Error::Invalid);
        }
        let answer = self
            .call_bytes(
                Method::POST,
                Self::endpoint(&format!("{phone}/media"))?,
                credentials.access_token.as_str(),
                upload.headers,
                upload.body,
            )
            .await?;
        let id = answer["id"]
            .as_str()
            .filter(|id| valid_meta_id(id))
            .ok_or(Error::Unconfirmed)?;
        let mut metadata = upload.metadata;
        metadata["id"] = id.into();
        Ok(UploadedMedia {
            phone: phone.into(),
            kind: upload.kind,
            metadata,
        })
    }

    pub(crate) async fn send_group_media(
        &self,
        credentials: &WhatsappCredentials,
        reply: &cmaeg_core::channel::whatsapp::WhatsappGroupReply,
        uploaded: UploadedMedia,
    ) -> Result<String, Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        if uploaded.phone != reply.phone_number_id
            || uploaded.kind == "sticker"
            || uploaded.metadata.get("voice").is_some()
            || credentials.validate().is_err()
        {
            return Err(Error::Invalid);
        }
        let mut body = json!({"messaging_product":"whatsapp","recipient_type":"group","to":reply.group_id,"context":{"message_id":reply.message_id},"type":uploaded.kind});
        body[uploaded.kind] = uploaded.metadata;
        let result = self
            .call(
                Method::POST,
                Self::endpoint(&format!("{}/messages", reply.phone_number_id))?,
                credentials.access_token.as_str(),
                Some(body),
            )
            .await?;
        Self::group_message_identity(&result, &reply.group_id)
    }

    /// Only an upload result from this phone can become a recipient message.
    pub(crate) async fn send_media(
        &self,
        credentials: &WhatsappCredentials,
        phone: &str,
        recipient: &str,
        parent: &str,
        uploaded: UploadedMedia,
    ) -> Result<String, Error> {
        if uploaded.phone != phone
            || !valid_meta_id(phone)
            || !valid_meta_id(recipient)
            || !valid_message_id(parent)
            || credentials.validate().is_err()
        {
            return Err(Error::Invalid);
        }
        let mut body = json!({"messaging_product":"whatsapp", "recipient_type":"individual",
            "to":recipient, "context":{"message_id":parent}, "type":uploaded.kind});
        body[uploaded.kind] = uploaded.metadata;
        let result = self
            .call(
                Method::POST,
                Self::endpoint(&format!("{phone}/messages"))?,
                credentials.access_token.as_str(),
                Some(body),
            )
            .await?;
        Self::message_identity(&result, recipient)
    }
}

#[cfg(test)]
#[path = "outbound_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "outbound_voice_tests.rs"]
mod voice_tests;
