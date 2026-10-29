//! Manager wire projection of the Gateway artifact claim. Gateway validates
//! its complete domain schema over HTTP; no Gateway implementation is linked.
use serde::{Deserialize, Serialize};

use layer1_public::rest::binding_media::BindingMediaError;

pub const MAX_CONTINUATION_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_CONTINUATION_FILE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaContinuationKind {
    Original,
    Transcript,
    TextExcerpt,
}
impl MediaContinuationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Transcript => "transcript",
            Self::TextExcerpt => "text_excerpt",
        }
    }
}

/// The descriptor always names the original uploaded file. For a derived kind,
/// content_sha256 hashes exact UTF-8 derived text; source_sha256 hashes the bytes
/// consumed by derivation. For Original both hashes name the original bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaArtifactClaim {
    pub attachment_index: u8,
    pub kind: MediaContinuationKind,
    pub content_sha256: String,
    pub source_sha256: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    /// None for an upload; required for a queued supplement. Hashes the complete
    /// canonical input, including the Manager-verified persisted file path.
    pub text_sha256: Option<String>,
}
impl MediaArtifactClaim {
    pub fn validate(&self) -> Result<(), BindingMediaError> {
        if self.attachment_index >= 32
            || !valid_digest(&self.content_sha256)
            || !valid_digest(&self.source_sha256)
            || self
                .text_sha256
                .as_deref()
                .is_some_and(|v| !valid_digest(v))
            || self.size_bytes > MAX_CONTINUATION_FILE_BYTES
            || self.name.is_empty()
            || self.name.len() > 120
            || self.name.starts_with('.')
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || self.media_type.len() > 127
            || !self.media_type.split_once('/').is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes()
                        .chain(b.bytes())
                        .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&c))
            })
            || self.kind == MediaContinuationKind::Original
                && self.content_sha256 != self.source_sha256
        {
            return Err(BindingMediaError::Invalid);
        }
        Ok(())
    }

    pub fn original(&self) -> Self {
        Self {
            kind: MediaContinuationKind::Original,
            content_sha256: self.source_sha256.clone(),
            text_sha256: None,
            ..self.clone()
        }
    }

    pub fn validate_derived_text(&self, text: &str) -> Result<(), BindingMediaError> {
        self.validate()?;
        let valid = match self.kind {
            MediaContinuationKind::Original => text.is_empty(),
            _ => {
                !text.trim().is_empty()
                    && text.len() <= MAX_CONTINUATION_TEXT_BYTES
                    && sha256(text.as_bytes()) == self.content_sha256
            }
        };
        if !valid {
            return Err(BindingMediaError::Invalid);
        }
        Ok(())
    }
}

/// Both services render this exact input after a successful original-file
/// receipt. Manager supplies the path from its own durable verified receipt.
pub fn supplement_text(
    artifact: &MediaArtifactClaim,
    path: &str,
    derived_text: &str,
) -> Result<String, BindingMediaError> {
    artifact.validate_derived_text(derived_text)?;
    if !path.starts_with("/home/attachments/")
        || path.len() > 1024
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(BindingMediaError::Invalid);
    }
    let mut text = format!(
        "Attachment available: {}\nPath: {}\nMedia type: {}\nSize: {} bytes\nSource SHA-256: {}",
        artifact.name, path, artifact.media_type, artifact.size_bytes, artifact.source_sha256
    );
    match artifact.kind {
        MediaContinuationKind::Original => {}
        MediaContinuationKind::Transcript => text.push_str("\n\nTranscript:\n"),
        MediaContinuationKind::TextExcerpt => text.push_str("\n\nText excerpt:\n"),
    }
    text.push_str(derived_text);
    Ok(text)
}

pub fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Digest the complete original retained Activity without dropping any fields.
pub(super) fn activity_digest(value: serde_json::Value) -> String {
    fn sort(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(object) => {
                let sorted = object
                    .into_iter()
                    .map(|(key, value)| (key, sort(value)))
                    .collect::<std::collections::BTreeMap<_, _>>();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(sort).collect())
            }
            other => other,
        }
    }
    sha256(&serde_json::to_vec(&sort(value)).expect("JSON serialization"))
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
