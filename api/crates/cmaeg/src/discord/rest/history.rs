//! Read one exact original-conversation page; all text is untrusted reference data.
use super::{DiscordRest, RestError};
use crate::discord::{history_authority::Authority, thread_authority::human_message};
use cmaeg_core::channel::Secret;
use serde_json::Value;

impl DiscordRest {
    pub(crate) async fn check_history_origin(
        &self,
        token: &Secret,
        authority: &Authority,
    ) -> Result<(), RestError> {
        let expected = &authority.origin;
        let value = self.thread_channel(token, &expected.channel_id).await?;
        if !expected.valid()
            || value["id"].as_str() != Some(expected.channel_id.as_str())
            || value["type"].as_u64() != Some(u64::from(expected.channel_type))
            || value["guild_id"].as_str() != expected.guild_id.as_deref()
            || (expected.guild_id.is_none() && !value["guild_id"].is_null())
            || (matches!(expected.channel_type, 10..=12)
                && value["parent_id"].as_str() != expected.parent_id.as_deref())
        {
            return Err(RestError::Rejected);
        }
        let anchor = self
            .message(token, &expected.channel_id, &authority.message)
            .await?;
        if !human_message(&anchor, &authority.activity.actor.id)
            || anchor
                .get("guild_id")
                .filter(|v| !v.is_null())
                .is_some_and(|v| v.as_str() != expected.guild_id.as_deref())
        {
            return Err(RestError::Rejected);
        }
        Ok(())
    }

    pub(crate) async fn history_rows(
        &self,
        token: &Secret,
        channel: &str,
        before: &str,
    ) -> Result<Value, RestError> {
        if !super::valid_id(channel) || !super::valid_id(before) {
            return Err(RestError::Rejected);
        }
        self.ready(token)?;
        let mut url = self.url(&format!("channels/{channel}/messages"))?;
        url.query_pairs_mut()
            .append_pair("before", before)
            .append_pair("limit", "26");
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        // Shared decoder caps the entire provider JSON at 1 MiB and observes cooldowns.
        self.decode(response, token, false).await
    }
}
