//! One voice-only multipart POST; uncertain transport never retries the send.
use super::{valid_id, DiscordRest, PreparedFile, RestError};
use crate::discord::voice::{Metadata, PreparedVoice, FLAG};
use cmaeg_core::channel::Secret;
use serde_json::{json, Value};

impl DiscordRest {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_voice(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        original: &str,
        prepared: &PreparedVoice,
    ) -> Result<String, RestError> {
        if ![bot, channel, original].into_iter().all(valid_id) {
            return Err(RestError::Rejected);
        }
        let file = prepared.file();
        let metadata = prepared.metadata();
        self.ready(token)?;
        self.verify_bot(token, bot).await?;
        self.ready(token)?;
        let payload = json!({
            "flags": FLAG,
            "allowed_mentions":{"parse":[],"replied_user":false},
            "message_reference":{"message_id":original,"channel_id":channel,"fail_if_not_exists":true},
            "attachments":[{"id":0,"filename":file.filename,
                "duration_secs":metadata.duration_secs,"waveform":metadata.waveform}]
        });
        let file_part = reqwest::multipart::Part::bytes(file.bytes.clone())
            .file_name(file.filename.clone())
            .mime_str(&file.mime)
            .map_err(|_| RestError::Rejected)?;
        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload.to_string())
            .part("files[0]", file_part);
        let response = self
            .client
            .post(self.url(&format!("channels/{channel}/messages"))?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .multipart(form)
            .send()
            .await
            .map_err(|_| RestError::Unknown)?;
        let body = self.decode(response, token, true).await?;
        confirmed(&body, bot, channel, original, file, metadata)
    }
}

fn confirmed(
    body: &Value,
    bot: &str,
    channel: &str,
    original: &str,
    file: &PreparedFile,
    metadata: &Metadata,
) -> Result<String, RestError> {
    let files = body["attachments"].as_array().ok_or(RestError::Unknown)?;
    if body["channel_id"].as_str() != Some(channel)
        || body["author"]["id"].as_str() != Some(bot)
        || body["author"]["bot"] != true
        || body["flags"].as_u64() != Some(FLAG)
        || body["content"] != ""
        || body["message_reference"]["message_id"].as_str() != Some(original)
        || body["message_reference"]["channel_id"].as_str() != Some(channel)
        || !matches!(body["type"].as_u64(), Some(0 | 19))
        || files.len() != 1
        || !files[0]["id"].as_str().is_some_and(valid_id)
        || files[0]["filename"].as_str() != Some(file.filename.as_str())
        || files[0]["content_type"].as_str() != Some(file.mime.as_str())
        || files[0]["size"].as_u64() != Some(file.bytes.len() as u64)
        || files[0]["duration_secs"].as_f64() != Some(metadata.duration_secs)
        || files[0]["waveform"].as_str() != Some(metadata.waveform.as_str())
        || ["embeds", "components", "sticker_items"]
            .into_iter()
            .any(|key| {
                body.get(key)
                    .is_some_and(|value| value.as_array().is_none_or(|values| !values.is_empty()))
            })
        || body.get("poll").is_some_and(|value| !value.is_null())
    {
        return Err(RestError::Unknown);
    }
    body["id"]
        .as_str()
        .filter(|id| valid_id(id) && *id != original)
        .map(str::to_owned)
        .ok_or(RestError::Unknown)
}
