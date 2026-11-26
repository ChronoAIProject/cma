//! Telegram platform transport. Errors and tracing never contain token-bearing URLs.
mod effects;
mod files;
mod management;
pub use management::WebhookError;
mod media;
pub(crate) mod media_card_edit;
pub(crate) mod media_edit;
mod media_presentation;
pub(crate) mod structured;
#[cfg(test)]
pub(crate) mod structured_tests;
pub(crate) use media::{album_supported, media_token_matches, AlbumUpload, MediaUpload};
#[cfg(test)]
mod media_edit_tests;
#[cfg(test)]
pub(crate) mod media_fixture;
#[cfg(test)]
mod media_tests;
pub(crate) mod messaging_compat;
#[cfg(test)]
mod tests;
pub(crate) mod topics;
mod transport;
use cmaeg_core::channel::Secret;
pub use files::{FileError, TelegramFile};
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE},
    Client, Url,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
pub use transport::{HttpTransport, TelegramTransport};

#[derive(Clone)]
pub struct TelegramClient {
    /// File downloads stream their body to the caller, so they keep the
    /// client; every Bot API call goes through `transport`.
    http: Client,
    transport: Arc<dyn TelegramTransport>,
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
            transport: Arc::new(HttpTransport { http: http.clone() }),
            http,
            origin,
            timeout,
        })
    }
    /// Send Bot API calls through `transport` instead of HTTP, as tests
    /// answering in process do. File downloads still use HTTP.
    pub fn with_transport(mut self, transport: Arc<dyn TelegramTransport>) -> Self {
        self.transport = transport;
        self
    }
    pub fn production(timeout: Duration) -> Result<Self, crate::ConfigError> {
        Self::new("https://api.telegram.org", timeout)
    }
    /// Sends one Bot API call and returns its HTTP status with the parsed body.
    ///
    /// Callers that must read Telegram's own `description`, for example to
    /// tell a too-large file from other refusals, use this form; everything
    /// else uses `call`, which maps the outcome onto [`Error`].
    async fn call_raw(
        &self,
        token: &Secret,
        method: &str,
        body: &Value,
    ) -> Result<(reqwest::StatusCode, Value), Error> {
        let value = token.as_str();
        if !valid_token(value) {
            return Err(Error::Rejected);
        }
        let endpoint = self
            .origin
            .join(&format!("/bot{value}/{method}"))
            .map_err(|_| Error::Rejected)?;
        let Ok(body) = serde_json::to_vec(body) else {
            return Err(Error::Unconfirmed);
        };
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let reply = self
            .transport
            .post(endpoint, headers, body, self.timeout)
            .await
            .map_err(|_| Error::Unconfirmed)?;
        let status = reply.status;
        let bytes = reply.body.map_err(|_| Error::Unconfirmed)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unconfirmed)?;
        Ok((status, value))
    }
    async fn call(&self, token: &Secret, method: &str, body: &Value) -> Result<Value, Error> {
        let (status, value) = self.call_raw(token, method, body).await?;
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
        let value = self
            .send_message_value(token, chat_id, thread_id, text, markup)
            .await?;
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
    /// A split-text reference grants authority for this exact chat and topic.
    pub(crate) async fn send_confirmed_text_part(
        &self,
        token: &Secret,
        chat_id: &str,
        thread_id: Option<&str>,
        text: &str,
    ) -> Result<String, Error> {
        let value = self
            .send_message_value(token, chat_id, thread_id, text, None)
            .await?;
        media::message_identity(&value, chat_id, thread_id)
    }
    async fn send_message_value(
        &self,
        token: &Secret,
        chat_id: &str,
        thread_id: Option<&str>,
        text: &str,
        markup: Option<&Value>,
    ) -> Result<Value, Error> {
        let mut body = json!({"chat_id":chat_id,"text":text});
        if let Some(thread_id) = thread_id {
            body["message_thread_id"] = thread_id.into();
        }
        if let Some(markup) = markup {
            body["reply_markup"] = markup.clone();
        }
        self.call(token, "sendMessage", &body).await
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
/// `<positive bot ID>:<key>` with a key of URL-safe characters, so a token
/// can never change the path it is embedded in.
fn valid_token(value: &str) -> bool {
    value.split_once(':').is_some_and(|(id, key)| {
        id.parse::<i64>().is_ok_and(|id| id > 0)
            && !key.is_empty()
            && key.len() <= 256
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    })
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

/// Separate source opt-ins never change retained ordinary messaging custody.
pub(crate) fn messaging_source(
    source: &cmaeg_core::channel::Source,
) -> cmaeg_core::channel::Source {
    let mut source = source.clone();
    if let cmaeg_core::channel::Source::Telegram {
        topic_lifecycle,
        topic_creation,
        member_join_chat_ids,
        ..
    } = &mut source
    {
        *topic_lifecycle = false;
        *topic_creation = false;
        member_join_chat_ids.clear();
    }
    source
}
