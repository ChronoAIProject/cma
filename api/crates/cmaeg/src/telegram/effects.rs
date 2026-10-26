//! Single-attempt native message effects. Telegram remains the authority on
//! message age, administrator rights and allowed custom emoji reactions.
use super::{Error, TelegramClient};
use cmaeg_core::{channel::Secret, message_effect::MessageReactionRequest};
use serde_json::json;

impl TelegramClient {
    pub async fn delete_message(
        &self,
        token: &Secret,
        chat: &str,
        message: &str,
    ) -> Result<(), Error> {
        let result = self
            .call(
                token,
                "deleteMessage",
                &json!({"chat_id":chat,"message_id":message}),
            )
            .await?;
        if result == true {
            Ok(())
        } else {
            Err(Error::Unconfirmed)
        }
    }
    pub async fn set_message_reaction(
        &self,
        token: &Secret,
        chat: &str,
        message: &str,
        request: &MessageReactionRequest,
    ) -> Result<(), Error> {
        request.validate().map_err(|_| Error::Rejected)?;
        let result = self.call(token, "setMessageReaction", &json!({"chat_id":chat,"message_id":message,"reaction":request.reaction,"is_big":request.is_big})).await?;
        if result == true {
            Ok(())
        } else {
            Err(Error::Unconfirmed)
        }
    }
}
