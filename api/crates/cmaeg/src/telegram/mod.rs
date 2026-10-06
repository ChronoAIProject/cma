//! Telegram platform transport. Errors and tracing never contain token-bearing URLs.
mod management;
use cmaeg_core::channel::Secret;
use reqwest::{Client, Url};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone)]
pub struct TelegramClient {
    http: Client,
    origin: Url,
    timeout: Duration,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Rejected,
    RateLimited(u64),
    Unconfirmed,
}
#[derive(Debug, Clone)]
pub struct BotIdentity {
    pub id: String,
    pub username: Option<String>,
    pub first_name: String,
    pub can_manage_bots: bool,
}
impl TelegramClient {
    pub fn new(origin: &str, timeout: Duration) -> Result<Self, crate::ConfigError> {
        let origin = crate::config::validate_origin("telegram_api_origin", origin)?;
        let origin = Url::parse(&origin).map_err(|_| config_error())?;
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout)
            .build()
            .map_err(|_| config_error())?;
        Ok(Self {
            http,
            origin,
            timeout,
        })
    }
    pub fn production(timeout: Duration) -> Result<Self, crate::ConfigError> {
        Self::new("https://api.telegram.org", timeout)
    }
    async fn call(&self, token: &Secret, method: &str, body: &Value) -> Result<Value, Error> {
        let value = token.as_str();
        let valid = value.split_once(':').is_some_and(|(id, key)| {
            id.parse::<i64>().is_ok_and(|id| id > 0)
                && !key.is_empty()
                && key.len() <= 256
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        });
        if !valid {
            return Err(Error::Rejected);
        }
        let endpoint = self
            .origin
            .join(&format!("/bot{value}/{method}"))
            .map_err(|_| Error::Rejected)?;
        let mut response = self
            .http
            .post(endpoint)
            .timeout(self.timeout)
            .json(body)
            .send()
            .await
            .map_err(|_| Error::Unconfirmed)?;
        if response.content_length().is_some_and(|size| size > 32_768) {
            return Err(Error::Unconfirmed);
        }
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unconfirmed)? {
            if bytes.len() + chunk.len() > 32_768 {
                return Err(Error::Unconfirmed);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unconfirmed)?;
        if status.as_u16() == 429 || value["error_code"] == 429 {
            return Err(Error::RateLimited(
                value["parameters"]["retry_after"]
                    .as_u64()
                    .unwrap_or(1)
                    .clamp(1, 3600),
            ));
        }
        if !status.is_success() || value["ok"] != true {
            return Err(if status.is_client_error() {
                Error::Rejected
            } else {
                Error::Unconfirmed
            });
        }
        value.get("result").cloned().ok_or(Error::Unconfirmed)
    }
    pub async fn send_message(
        &self,
        token: &Secret,
        chat_id: &str,
        thread_id: Option<&str>,
        text: &str,
        markup: Option<&Value>,
    ) -> Result<String, Error> {
        let mut body = json!({"chat_id":chat_id,"text":text});
        if let Some(thread_id) = thread_id {
            body["message_thread_id"] = thread_id.into();
        }
        if let Some(markup) = markup {
            body["reply_markup"] = markup.clone();
        }
        let value = self.call(token, "sendMessage", &body).await?;
        if value["chat"]["id"]
            .as_i64()
            .map(|id| id.to_string())
            .as_deref()
            != Some(chat_id)
        {
            return Err(Error::Unconfirmed);
        }
        positive(&value["message_id"])
    }
    pub async fn edit_message(
        &self,
        token: &Secret,
        chat_id: &str,
        message_id: &str,
        text: &str,
        markup: &Value,
    ) -> Result<(), Error> {
        let value = self.call(token, "editMessageText", &json!({"chat_id":chat_id,"message_id":message_id,"text":text,"reply_markup":markup})).await?;
        if value["chat"]["id"]
            .as_i64()
            .map(|id| id.to_string())
            .as_deref()
            != Some(chat_id)
            || positive(&value["message_id"])?.as_str() != message_id
        {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
    pub async fn typing(
        &self,
        token: &Secret,
        chat_id: &str,
        thread_id: Option<&str>,
    ) -> Result<(), Error> {
        let mut body = json!({"chat_id":chat_id,"action":"typing"});
        if let Some(thread) = thread_id {
            body["message_thread_id"] = thread.into();
        }
        if self.call(token, "sendChatAction", &body).await? != true {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
    pub async fn acknowledge_callback(
        &self,
        token: &Secret,
        callback_id: &str,
    ) -> Result<(), Error> {
        if self
            .call(
                token,
                "answerCallbackQuery",
                &json!({"callback_query_id":callback_id}),
            )
            .await?
            != true
        {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
fn positive(value: &Value) -> Result<String, Error> {
    value
        .as_i64()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .ok_or(Error::Unconfirmed)
}
fn config_error() -> crate::ConfigError {
    crate::ConfigError::InvalidManagement {
        detail: "cannot configure Telegram transport".into(),
    }
}
pub fn route(bot_id: &str, chat_id: &str) -> String {
    format!("telegram:{bot_id}:{chat_id}")
}
pub fn route_chat<'a>(route: &'a str, bot_id: &str) -> Option<&'a str> {
    let (bot, chat) = route.strip_prefix("telegram:")?.split_once(':')?;
    (bot == bot_id
        && chat
            .parse::<i64>()
            .is_ok_and(|id| id != 0 && id.to_string() == chat))
    .then_some(chat)
}
