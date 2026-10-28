//! One file and its complete successor controls share a single multipart PATCH.
use super::{media_edit::PreparedEdit, DiscordRest, RestError};
use crate::{media::outbound::PreparedFile, native_media::model::Snapshot};
use serde_json::{json, Value};

impl DiscordRest {
    pub(crate) async fn prepare_media_card_replacement(
        &self,
        text: &str,
        components: &Value,
        before: &Snapshot,
        file: PreparedFile,
    ) -> Result<PreparedEdit, RestError> {
        let spoiler = super::media_presentation::spoiler(before.presentation.as_ref())?;
        if text.encode_utf16().count() > 2000
            || components.as_array().is_none_or(|rows| rows.len() > 5)
            || (file.filename.starts_with("SPOILER_") && !spoiler)
        {
            return Err(RestError::Rejected);
        }
        let payload = json!({
            "content": text,
            "components": components,
            "allowed_mentions": {"parse": [], "replied_user": false},
            "attachments": [{
                "id": 0,
                "filename": file.filename,
                "description": file.caption.as_deref().unwrap_or(""),
                "is_spoiler": spoiler,
            }],
        });
        self.prepare_media_payload(payload, Some(file)).await
    }
}
