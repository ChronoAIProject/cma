//! Explicit native media requests; legacy attachment blocks keep their wire meaning.
use crate::{require_string, ValidationError};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_NATIVE_ATTACHMENTS: usize = 4;
/// Aggregate decoded inline content, leaving room in the existing 128 KiB tool request.
pub const MAX_NATIVE_INLINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeMediaKind {
    Image,
    File,
    Audio,
    Video,
    /// Sticker presentation; only destinations advertising it may issue one.
    Sticker,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeMediaSource {
    /// The native adapter or NyxID fetches public HTTPS with bounded, pinned-address policy.
    Url { url: String },
    /// Standard padded base64, with at most 64 KiB decoded across the request.
    Base64 { data: String },
}

#[derive(Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeAttachment {
    pub kind: NativeMediaKind,
    pub source: NativeMediaSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

impl std::fmt::Debug for NativeMediaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeMediaSource([REDACTED])")
    }
}
impl std::fmt::Debug for NativeAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeAttachment")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

pub fn validate(items: &[NativeAttachment]) -> Result<(), ValidationError> {
    let invalid = |pointer: &str, message: &str| ValidationError::new(pointer, message);
    if items.len() > MAX_NATIVE_ATTACHMENTS {
        return Err(invalid(
            "/native_attachments",
            "at most four native attachments",
        ));
    }
    let mut total = 0usize;
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/native_attachments/{index}");
        for (field, value, max) in [
            ("filename", item.filename.as_deref(), 255),
            ("mime_type", item.mime_type.as_deref(), 255),
            ("caption", item.caption.as_deref(), 1024),
        ] {
            if let Some(value) = value {
                require_string(value, &format!("{pointer}/{field}"), 1, max)?;
                if value.chars().any(crate::activity::forbidden_text_char) {
                    return Err(invalid(
                        &pointer,
                        "media metadata contains control characters",
                    ));
                }
            }
        }
        if item
            .filename
            .as_deref()
            .is_some_and(|name| name.contains(['/', '\\']) || matches!(name, "." | ".."))
        {
            return Err(invalid(
                &pointer,
                "filename must be a single path component",
            ));
        }
        if item
            .mime_type
            .as_deref()
            .is_some_and(|mime| !crate::activity::mime_restricted_name(mime))
        {
            return Err(invalid(&pointer, "invalid media MIME type"));
        }
        match &item.source {
            NativeMediaSource::Url { url } => {
                require_string(url, &format!("{pointer}/source/url"), 1, 8192)?;
                let parsed =
                    url::Url::parse(url).map_err(|_| invalid(&pointer, "invalid media URL"))?;
                if parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                    || url.chars().any(char::is_whitespace)
                {
                    return Err(invalid(
                        &pointer,
                        "media URL must be public HTTPS without userinfo or fragment",
                    ));
                }
            }
            NativeMediaSource::Base64 { data } => {
                // Check before allocation; validate canonical padding and actual decoded length.
                if data.is_empty() || data.len() > MAX_NATIVE_INLINE_BYTES.div_ceil(3) * 4 {
                    return Err(invalid(
                        &pointer,
                        "inline media exceeds the decoded byte limit",
                    ));
                }
                let bytes = STANDARD
                    .decode(data)
                    .map_err(|_| invalid(&pointer, "invalid standard base64 media"))?;
                total = total.saturating_add(bytes.len());
                if bytes.is_empty() || total > MAX_NATIVE_INLINE_BYTES {
                    return Err(invalid(
                        &pointer,
                        "inline media exceeds the aggregate decoded byte limit",
                    ));
                }
            }
        }
    }
    Ok(())
}
