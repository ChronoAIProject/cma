//! Original-author message PATCH with complete bounded component replacement.
use super::{valid_id, DiscordRest, RestError};
use cmaeg_core::channel::Secret;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

impl DiscordRest {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn edit_components(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        message: &str,
        text: &str,
        components: &Value,
    ) -> Result<(), RestError> {
        if ![bot, channel, message].into_iter().all(valid_id)
            || text.encode_utf16().count() > 2000
            || components
                .as_array()
                .is_none_or(|rows| rows.len() > 5 || (rows.is_empty() && text.is_empty()))
        {
            return Err(RestError::Rejected);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        self.ready(token)?;
        tokio::time::timeout_at(deadline, self.verify_bot(token, bot))
            .await
            .map_err(|_| RestError::Unavailable)??;
        self.ready(token)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RestError::Unavailable);
        }
        let response = self
            .client
            .patch(self.url(&format!("channels/{channel}/messages/{message}"))?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .timeout(remaining)
            .json(&json!({"content":text,"components":components,
                "allowed_mentions":{"parse":[],"replied_user":false}}))
            .send()
            .await
            .map_err(|_| RestError::Unknown)?;
        let body = self.decode(response, token, true).await?;
        if body["id"].as_str() != Some(message)
            || body["channel_id"].as_str() != Some(channel)
            || body["author"]["id"].as_str() != Some(bot)
            || body["content"].as_str() != Some(text)
            || !matches_components(components, &body["components"])
        {
            return Err(RestError::Unknown);
        }
        Ok(())
    }
}
/// Discord may add numeric component IDs and omit false defaults. Match every
/// authored field and every array member, rejecting disabled/retargeted controls.
fn matches_components(expected: &Value, actual: &Value) -> bool {
    match expected {
        Value::Array(values) => actual.as_array().is_some_and(|seen| {
            values.len() == seen.len()
                && values
                    .iter()
                    .zip(seen)
                    .all(|(a, b)| matches_components(a, b))
        }),
        Value::Object(fields) => actual.as_object().is_some_and(|seen| {
            !seen
                .get("disabled")
                .is_some_and(|v| v != &Value::Bool(false))
                && fields.iter().all(|(key, value)| match seen.get(key) {
                    Some(found) => matches_components(value, found),
                    None => key == "default" && value == &Value::Bool(false),
                })
        }),
        _ => expected == actual,
    }
}
