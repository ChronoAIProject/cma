//! Complete final membership and successor controls share one bounded PATCH.
use super::{media_edit::PreparedEdit, DiscordRest, RestError};
use crate::{media::outbound::PreparedFile, native_media::model::Snapshot};
use serde_json::{json, Value};

impl DiscordRest {
    pub(crate) async fn prepare_media_card_replacement(
        &self,
        text: &str,
        components: &Value,
        before: &[Snapshot],
        files: Vec<PreparedFile>,
    ) -> Result<PreparedEdit, RestError> {
        if before.is_empty()
            || files.len() > cmaeg_core::message::MAX_NATIVE_ATTACHMENTS
            || text.encode_utf16().count() > 2000
            || components.as_array().is_none_or(|rows| rows.len() > 5)
        {
            return Err(RestError::Rejected);
        }
        let mut attachments = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let spoiler = match before.get(index) {
                Some(previous) => {
                    super::media_presentation::spoiler(previous.presentation.as_ref())?
                }
                None => file.filename.starts_with("SPOILER_"),
            };
            if file.filename.starts_with("SPOILER_") && !spoiler {
                return Err(RestError::Rejected);
            }
            attachments.push(json!({"id":index,"filename":file.filename,
                "description":file.caption.as_deref().unwrap_or(""),"is_spoiler":spoiler}));
        }
        let payload = json!({"content":text,"components":components,
            "allowed_mentions":{"parse":[],"replied_user":false},"attachments":attachments});
        self.prepare_media_payload_set(payload, files).await
    }
}
