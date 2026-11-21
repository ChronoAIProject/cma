//! Shared artifact claims and exact supplement rendering. These values are
//! comparisons against durable Gateway obligations, never authority by themselves.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ValidationError;

pub const MAX_CONTINUATION_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_CONTINUATION_FILE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
    pub fn validate(&self) -> Result<(), ValidationError> {
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
            return Err(ValidationError::new(
                "/artifact",
                "invalid media artifact claim",
            ));
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

    pub fn validate_derived_text(&self, text: &str) -> Result<(), ValidationError> {
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
            return Err(ValidationError::new(
                "/derived_text",
                "artifact text does not match",
            ));
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
) -> Result<String, ValidationError> {
    artifact.validate_derived_text(derived_text)?;
    if !path.starts_with("/home/attachments/")
        || path.len() > 1024
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(ValidationError::new(
            "/path",
            "invalid persisted attachment path",
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    fn original() -> MediaArtifactClaim {
        MediaArtifactClaim {
            attachment_index: 0,
            kind: MediaContinuationKind::Original,
            content_sha256: sha256(b"file"),
            source_sha256: sha256(b"file"),
            name: "voice.ogg".into(),
            media_type: "audio/ogg".into(),
            size_bytes: 4,
            text_sha256: None,
        }
    }
    #[test]
    fn rendering_preserves_exact_text_and_distinct_artifact_slots() {
        let original = original();
        let path = "/home/attachments/agt_fixture/att_fixture/voice.ogg";
        let file = supplement_text(&original, path, "").unwrap();
        let mut transcript = original.clone();
        transcript.kind = MediaContinuationKind::Transcript;
        transcript.content_sha256 = sha256(b" exact text\n");
        let text = supplement_text(&transcript, path, " exact text\n").unwrap();
        assert_eq!(text, format!("{file}\n\nTranscript:\n exact text\n"));
        assert_eq!(transcript.original(), original);
        assert!(supplement_text(&transcript, path, "exact text").is_err());
        assert!(supplement_text(&original, path, "unattested text").is_err());
    }
    #[test]
    fn invalid_paths_descriptors_and_changed_source_are_refused() {
        let mut claim = original();
        assert!(supplement_text(&claim, "/home/attachments/../escape", "").is_err());
        claim.content_sha256 = sha256(b"different");
        assert!(claim.validate().is_err());
        claim = original();
        claim.name = "../voice.ogg".into();
        assert!(claim.validate().is_err());
        claim = original();
        claim.attachment_index = 32;
        assert!(claim.validate().is_err());
    }
}
