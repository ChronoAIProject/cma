//! Explicit update-only patches. Omission preserves; empty strings clear; null refuses.
use super::{NativeAttachment, NativeMediaKind, NativeMediaSource};
use crate::{require_string, ValidationError};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeMediaUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub replacement: Option<NativeMediaReplacement>,
    /// Telegram visible caption; Discord attachment description (alt text).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub caption: Option<String>,
    /// Discord visible message content. Unsupported for Telegram media updates.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub text: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeMediaReplacement {
    pub kind: NativeMediaKind,
    pub source: NativeMediaSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}
impl NativeMediaReplacement {
    pub fn attachment(&self) -> NativeAttachment {
        NativeAttachment {
            kind: self.kind,
            source: self.source.clone(),
            filename: self.filename.clone(),
            mime_type: self.mime_type.clone(),
            caption: None,
        }
    }
}
impl NativeMediaUpdate {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.replacement.is_none() && self.caption.is_none() && self.text.is_none() {
            return Err(ValidationError::new(
                "/native_media_update",
                "at least one update is required",
            ));
        }
        if let Some(item) = &self.replacement {
            super::validate_native_attachments(&[item.attachment()])?;
            if item.kind == NativeMediaKind::Sticker {
                return Err(ValidationError::new(
                    "/native_media_update/replacement",
                    "sticker edits are unsupported",
                ));
            }
        }
        for (field, value, limit) in [("caption", &self.caption, 1024), ("text", &self.text, 2000)]
        {
            if let Some(value) = value {
                require_string(
                    value,
                    &format!("/native_media_update/{field}"),
                    0,
                    limit * 4,
                )?;
                if value.encode_utf16().count() > limit
                    || value.chars().any(crate::activity::forbidden_text_char)
                {
                    return Err(ValidationError::new(
                        format!("/native_media_update/{field}"),
                        "text exceeds UTF-16 bound or contains controls",
                    ));
                }
            }
        }
        Ok(())
    }
}
pub(super) fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
