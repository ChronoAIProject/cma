//! One explicitly requested typing indicator for an authenticated received message.
use super::{Client, Error};
use cmaeg_core::channel::{
    whatsapp::{valid_message_id, valid_meta_id},
    WhatsappCredentials,
};
use reqwest::Method;
use serde_json::json;

impl Client {
    /// Meta marks this message read and displays text typing until a response or
    /// 25 seconds. The caller supplies retained original-message authority.
    /// A lost response is never automatically retried or reported as success.
    pub(crate) async fn typing(
        &self,
        credentials: &WhatsappCredentials,
        phone: &str,
        message: &str,
    ) -> Result<(), Error> {
        if credentials.validate().is_err() || !valid_meta_id(phone) || !valid_message_id(message) {
            return Err(Error::Invalid);
        }
        let result = self
            .call(
                Method::POST,
                Self::endpoint(&format!("{phone}/messages"))?,
                credentials.access_token.as_str(),
                Some(json!({
                    "messaging_product": "whatsapp",
                    "status": "read",
                    "message_id": message,
                    "typing_indicator": { "type": "text" }
                })),
            )
            .await?;
        if result["success"].as_bool() != Some(true) {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
