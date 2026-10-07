//! Attachments carried by a message activity.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{require_string, ValidationError};

pub const MAX_ATTACHMENTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub reference: String,
    pub kind: AttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    File,
    Audio,
    Video,
    Other,
}

/// Validate a message activity's attachments, reporting pointers relative to
/// the activity so callers see the same errors as from `Activity::validate`.
pub(super) fn validate_message_attachments(
    attachments: &[Attachment],
) -> Result<(), ValidationError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(ValidationError::new(
            "/kind/attachments",
            "too many attachments",
        ));
    }
    for (index, attachment) in attachments.iter().enumerate() {
        require_string(
            &attachment.reference,
            &format!("/kind/attachments/{index}/reference"),
            1,
            4_096,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(reference: &str) -> Attachment {
        Attachment {
            reference: reference.into(),
            kind: AttachmentKind::File,
            filename: None,
            mime_type: None,
            size_bytes: None,
        }
    }

    #[test]
    fn accepts_up_to_the_attachment_limit() {
        validate_message_attachments(&[]).unwrap();
        validate_message_attachments(&vec![attachment("r"); MAX_ATTACHMENTS]).unwrap();
    }

    #[test]
    fn refuses_too_many_attachments_and_invalid_references() {
        let error =
            validate_message_attachments(&vec![attachment("r"); MAX_ATTACHMENTS + 1]).unwrap_err();
        assert_eq!(error.pointer, "/kind/attachments");
        assert_eq!(error.message, "too many attachments");
        let oversized = "x".repeat(4_097);
        for reference in ["", oversized.as_str()] {
            let error = validate_message_attachments(&[attachment("r"), attachment(reference)])
                .unwrap_err();
            assert_eq!(error.pointer, "/kind/attachments/1/reference");
        }
    }
}
