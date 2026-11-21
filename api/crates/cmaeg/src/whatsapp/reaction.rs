//! One native reaction on an exactly authorized received WhatsApp message.
use super::{Client, Error};
use cmaeg_core::{
    channel::{
        whatsapp::{valid_message_id, valid_meta_id},
        WhatsappCredentials,
    },
    message_effect::{MessageReactionRequest, TelegramReaction},
};
use reqwest::Method;
use serde_json::json;

impl Client {
    /// Send once; a successful response confirms API acceptance, not delivery.
    ///
    /// The caller must prove the current source/consumer, the original received
    /// message and its age, then acquire the durable exact-target mutation fence.
    /// This transport never resolves arbitrary IDs or retries an uncertain write.
    pub async fn send_reaction(
        &self,
        credentials: &WhatsappCredentials,
        phone: &str,
        recipient: &str,
        target: &str,
        request: &MessageReactionRequest,
    ) -> Result<String, Error> {
        if credentials.validate().is_err()
            || !valid_meta_id(phone)
            || !valid_meta_id(recipient)
            || !valid_message_id(target)
            || request.is_big
            || request.validate().is_err()
        {
            return Err(Error::Invalid);
        }
        // Meta's official Messages Object / Reaction Object documents an empty
        // emoji string as removal of this bot's reaction (see feature contract).
        let emoji = match request.reaction.as_slice() {
            [] => "",
            [TelegramReaction::Emoji { emoji }] => emoji.as_str(),
            _ => return Err(Error::Invalid),
        };
        let result = self
            .call(
                Method::POST,
                Self::endpoint(&format!("{phone}/messages"))?,
                credentials.access_token.as_str(),
                Some(json!({
                    "messaging_product": "whatsapp",
                    "recipient_type": "individual",
                    "to": recipient,
                    "type": "reaction",
                    "reaction": { "message_id": target, "emoji": emoji },
                })),
            )
            .await?;
        Self::message_identity(&result, recipient)
    }
}

#[cfg(test)]
#[path = "reaction_tests.rs"]
mod tests;
