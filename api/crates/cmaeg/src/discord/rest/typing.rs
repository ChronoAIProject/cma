//! Explicit typing on the original channel, including an original thread channel.
use super::{valid_id, DiscordRest, RestError};
use cmaeg_core::channel::Secret;
use reqwest::Method;

impl DiscordRest {
    /// The indicator expires after ten seconds. Verify the bot and recheck
    /// retained custody before the single write; never retry ambiguous writes.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn typing<F, Fut>(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        message: &str,
        actor: &str,
        check: F,
    ) -> Result<(), RestError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), RestError>>,
    {
        if !valid_id(bot)
            || !valid_id(channel)
            || !valid_id(message)
            || !valid_id(actor)
            || bot == actor
        {
            return Err(RestError::Rejected);
        }
        self.verify_media_bot(token, bot).await?;
        let original = self.message(token, channel, message).await?;
        if original["author"]["id"].as_str() != Some(actor)
            || ["bot", "system"].iter().any(|flag| {
                original["author"]
                    .get(*flag)
                    .is_some_and(|value| value.as_bool() != Some(false))
            })
            || original
                .get("webhook_id")
                .is_some_and(|value| !value.is_null())
            || !matches!(original["type"].as_u64(), Some(0 | 19))
        {
            return Err(RestError::Rejected);
        }
        check().await?;
        self.effect_request(
            token,
            Method::POST,
            self.url(&format!("channels/{channel}/typing"))?,
        )
        .await
    }
}
