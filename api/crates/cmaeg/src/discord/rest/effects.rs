//! Exact bot-message effects. The caller owns durable authorization/idempotency;
//! these transport primitives never retry a possibly accepted write.
use super::{valid_id, DiscordRest, RestError};
use cmaeg_core::{
    channel::Secret,
    message_effect::{MessageReactionRequest, TelegramReaction},
};
use reqwest::{Method, Url};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};

impl DiscordRest {
    pub async fn delete_message(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        message: &str,
    ) -> Result<(), RestError> {
        tokio::time::timeout(Duration::from_secs(5), async {
            self.own_message(token, bot, channel, message).await?;
            self.effect_request(
                token,
                Method::DELETE,
                self.url(&format!("channels/{channel}/messages/{message}"))?,
            )
            .await
        })
        .await
        .unwrap_or(Err(RestError::Unknown))
    }

    /// Replace only this bot's ordinary reactions. The public operation requests
    /// zero or one reaction, so retained bot reactions are removed individually;
    /// no moderator operation can clear another user's reaction.
    pub async fn set_message_reaction(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        message: &str,
        request: &MessageReactionRequest,
    ) -> Result<(), RestError> {
        if request.validate().is_err() || request.is_big {
            return Err(RestError::Rejected);
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let current = self.own_message(token, bot, channel, message).await?;
            let desired = match request.reaction.first() {
                None => None,
                Some(TelegramReaction::Emoji { emoji }) => Some(emoji.clone()),
                Some(TelegramReaction::CustomEmoji { custom_emoji_id }) => {
                    Some(self.custom_emoji(token, channel, custom_emoji_id).await?)
                }
            };
            let existing = own_reactions(&current)?;
            // Validate every exact destination before the first removal.
            for emoji in existing.iter().chain(desired.iter()) {
                self.reaction_url(channel, message, emoji)?;
            }
            let mut changed = false;
            for emoji in existing
                .iter()
                .filter(|emoji| Some(*emoji) != desired.as_ref())
            {
                let result = self
                    .effect_request(
                        token,
                        Method::DELETE,
                        self.reaction_url(channel, message, emoji)?,
                    )
                    .await;
                if let Err(error) = result {
                    return Err(if changed { RestError::Unknown } else { error });
                }
                changed = true;
            }
            if let Some(emoji) = desired.filter(|emoji| !existing.contains(emoji)) {
                if let Err(error) = self
                    .effect_request(
                        token,
                        Method::PUT,
                        self.reaction_url(channel, message, &emoji)?,
                    )
                    .await
                {
                    return Err(if changed { RestError::Unknown } else { error });
                }
            }
            Ok(())
        })
        .await
        .unwrap_or(Err(RestError::Unknown))
    }

    async fn own_message(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        message: &str,
    ) -> Result<Value, RestError> {
        if !valid_id(bot) || !valid_id(channel) || !valid_id(message) {
            return Err(RestError::Rejected);
        }
        self.verify_bot(token, bot).await?;
        let result = self.message(token, channel, message).await?;
        if result["author"]["id"].as_str() != Some(bot) {
            return Err(RestError::Rejected);
        }
        Ok(result)
    }

    async fn custom_emoji(
        &self,
        token: &Secret,
        channel: &str,
        emoji: &str,
    ) -> Result<String, RestError> {
        if !valid_id(emoji) {
            return Err(RestError::Rejected);
        }
        let channel_body = self
            .effect_metadata(token, &format!("channels/{channel}"))
            .await?;
        let guild = channel_body["guild_id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or(RestError::Rejected)?;
        if channel_body["id"].as_str() != Some(channel) {
            return Err(RestError::Rejected);
        }
        let value = self
            .effect_metadata(token, &format!("guilds/{guild}/emojis/{emoji}"))
            .await?;
        if value["id"].as_str() != Some(emoji) || value["available"].as_bool() == Some(false) {
            return Err(RestError::Rejected);
        }
        emoji_name(&value)
    }

    async fn effect_metadata(&self, token: &Secret, path: &str) -> Result<Value, RestError> {
        self.ready(token)?;
        let response = self
            .client
            .get(self.url(path)?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        self.decode(response, token, false).await
    }

    fn reaction_url(&self, channel: &str, message: &str, emoji: &str) -> Result<Url, RestError> {
        if matches!(emoji, "" | "." | "..") {
            return Err(RestError::Rejected);
        }
        let mut url = self.url(&format!("channels/{channel}/messages/{message}/reactions/"))?;
        url.path_segments_mut()
            .map_err(|_| RestError::Rejected)?
            .pop_if_empty()
            .push(emoji)
            .push("@me");
        Ok(url)
    }

    async fn effect_request(
        &self,
        token: &Secret,
        method: Method,
        url: Url,
    ) -> Result<(), RestError> {
        self.ready(token)?;
        let response = self
            .client
            .request(method, url)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unknown)?;
        // These Discord operations confirm with 204 and no JSON body. Still
        // consume the rate-limit headers before the next exact write.
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            if response
                .headers()
                .get("x-ratelimit-remaining")
                .is_some_and(|value| value.as_bytes() == b"0")
            {
                let delay = response
                    .headers()
                    .get("x-ratelimit-reset-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<f64>().ok())
                    .unwrap_or(1.0);
                self.cooldown(token, delay);
            }
            return Ok(());
        }
        self.decode(response, token, true).await?;
        Err(RestError::Unknown)
    }
}

fn own_reactions(message: &Value) -> Result<BTreeSet<String>, RestError> {
    let Some(reactions) = message.get("reactions") else {
        return Ok(BTreeSet::new());
    };
    let reactions = reactions.as_array().ok_or(RestError::Rejected)?;
    if reactions.len() > 64 {
        return Err(RestError::Rejected);
    }
    let mut result = BTreeSet::new();
    for reaction in reactions {
        match reaction["me"].as_bool() {
            Some(true) => {
                result.insert(emoji_name(&reaction["emoji"])?);
            }
            Some(false) => {}
            None => return Err(RestError::Rejected),
        }
    }
    Ok(result)
}

fn emoji_name(emoji: &Value) -> Result<String, RestError> {
    let name = emoji["name"]
        .as_str()
        .filter(|value| {
            !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
        })
        .ok_or(RestError::Rejected)?;
    match emoji.get("id").filter(|id| !id.is_null()) {
        None => Ok(name.into()),
        Some(value) => {
            let id = value
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or(RestError::Rejected)?;
            if name.len() > 32
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(RestError::Rejected);
            }
            Ok(format!("{name}:{id}"))
        }
    }
}
