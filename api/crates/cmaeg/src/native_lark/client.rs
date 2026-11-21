//! Regional, redirect-free API client. Only explicit success proves a send.
use crate::net::exchange::{read_bounded, HttpReply, TransportError};
use cmaeg_core::channel::{
    native_lark::safe_id, NativeLarkCredentials, NativeLarkIdentity, NativeLarkReply, Secret,
};
use futures_util::future::BoxFuture;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Method, Url,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Rejected,
    RateLimited,
    Unconfirmed,
}
pub trait Transport: Send + Sync {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>>;
}
struct HttpTransport(reqwest::Client, usize);
impl Transport for HttpTransport {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            let response = self
                .0
                .request(method, url)
                .headers(headers)
                .body(body)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .map_err(|_| TransportError::Unavailable)?;
            Ok(HttpReply {
                status: response.status(),
                headers: response.headers().clone(),
                body: read_bounded(response, self.1).await,
            })
        })
    }
}
#[derive(Clone)]
pub struct Client {
    transport: Arc<dyn Transport>,
    response_limit: usize,
}
impl Client {
    pub fn production() -> Result<Self, crate::ConfigError> {
        Self::bounded(64 * 1024)
    }
    pub(crate) fn history_reader() -> Result<Self, crate::ConfigError> {
        Self::bounded(1024 * 1024)
    }
    fn bounded(response_limit: usize) -> Result<Self, crate::ConfigError> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| crate::ConfigError::InvalidManagement {
                detail: "cannot configure native Lark client".into(),
            })?;
        Ok(Self {
            transport: Arc::new(HttpTransport(http, response_limit)),
            response_limit,
        })
    }
    pub fn with_transport(mut self, transport: Arc<dyn Transport>) -> Self {
        self.transport = transport;
        self
    }
    pub(super) async fn call(
        &self,
        identity: &NativeLarkIdentity,
        method: Method,
        path: &str,
        token: Option<&Secret>,
        body: Option<Value>,
    ) -> Result<Value, Error> {
        let bytes = body
            .map(|b| serde_json::to_vec(&b))
            .transpose()
            .map_err(|_| Error::Invalid)?
            .unwrap_or_default();
        // The official text endpoint allows 150 KB, including the double-encoded content.
        if bytes.len() > 150 * 1024 {
            return Err(Error::Invalid);
        }
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        self.call_bytes(identity, method, path, token, headers, bytes)
            .await
    }
    pub(super) async fn call_bytes(
        &self,
        identity: &NativeLarkIdentity,
        method: Method,
        path: &str,
        token: Option<&Secret>,
        mut headers: HeaderMap,
        bytes: Vec<u8>,
    ) -> Result<Value, Error> {
        identity.validate().map_err(|_| Error::Invalid)?;
        if bytes.len() as u64 > crate::media::outbound::MAX_OUTBOUND_BYTES + 16 * 1024 {
            return Err(Error::Invalid);
        }
        let url = Url::parse(&format!("{}{path}", identity.region.origin()))
            .map_err(|_| Error::Invalid)?;
        if let Some(token) = token {
            let mut value = HeaderValue::from_str(&format!("Bearer {}", token.as_str()))
                .map_err(|_| Error::Invalid)?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.transport.exchange(method, url, headers, bytes),
        )
        .await
        .map_err(|_| Error::Unconfirmed)?
        .map_err(|_| Error::Unconfirmed)?;
        if response.status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::RateLimited);
        }
        if response.status != reqwest::StatusCode::OK {
            return Err(Error::Unconfirmed);
        }
        let bytes = response.body.map_err(|_| Error::Unconfirmed)?;
        if bytes.len() > self.response_limit {
            return Err(Error::Unconfirmed);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unconfirmed)?;
        match value["code"].as_i64() {
            Some(0) => Ok(value),
            Some(_) => Err(Error::Rejected),
            None => Err(Error::Unconfirmed),
        }
    }
    pub(crate) async fn token(
        &self,
        identity: &NativeLarkIdentity,
        credentials: &NativeLarkCredentials,
    ) -> Result<Secret, Error> {
        credentials.validate().map_err(|_| Error::Invalid)?;
        let value = self
            .call(
                identity,
                Method::POST,
                "/open-apis/auth/v3/tenant_access_token/internal",
                None,
                Some(
                    json!({"app_id":identity.app_id,"app_secret":credentials.app_secret.as_str()}),
                ),
            )
            .await?;
        let token = value["tenant_access_token"]
            .as_str()
            .filter(|s| {
                s.len() >= 16 && s.len() <= 4096 && s.bytes().all(|b| (33..=126).contains(&b))
            })
            .ok_or(Error::Unconfirmed)?;
        if !value["expire"].as_u64().is_some_and(|v| v > 0 && v <= 7200) {
            return Err(Error::Unconfirmed);
        }
        Ok(Secret::new(token))
    }
    pub async fn verify_custody(
        &self,
        identity: &NativeLarkIdentity,
        credentials: &NativeLarkCredentials,
    ) -> Result<(), Error> {
        self.verified_token(identity, credentials).await.map(|_| ())
    }
    pub(crate) async fn verified_token(
        &self,
        identity: &NativeLarkIdentity,
        credentials: &NativeLarkCredentials,
    ) -> Result<Secret, Error> {
        let token = self.token(identity, credentials).await?;
        let value = self
            .call(
                identity,
                Method::GET,
                "/open-apis/bot/v3/info",
                Some(&token),
                None,
            )
            .await?;
        if value["bot"]["open_id"].as_str() != Some(&identity.bot_open_id) {
            return Err(Error::Rejected);
        }
        Ok(token)
    }
    pub(crate) async fn send_text(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        text: &str,
    ) -> Result<String, Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        if text.is_empty() || text.len() > 28 * 1024 {
            return Err(Error::Invalid);
        }
        self.send_content(token, reply, "text", json!({"text":text}))
            .await
    }
    pub(super) async fn send_content(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        kind: &str,
        content: Value,
    ) -> Result<String, Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        // Send to the retained chat so recalled originals remain replyable. Thread replies
        // use the retained original message; a platform rejection is never rerouted.
        let (path, body) = if reply.thread_id.is_some() {
            (
                format!("/open-apis/im/v1/messages/{}/reply", reply.message_id),
                json!({"msg_type":kind,"content":content.to_string(),"reply_in_thread":true}),
            )
        } else {
            (
                "/open-apis/im/v1/messages?receive_id_type=chat_id".into(),
                json!({"receive_id":reply.chat_id,"msg_type":kind,"content":content.to_string()}),
            )
        };
        let value = self
            .call(
                &reply.identity,
                Method::POST,
                &path,
                Some(token),
                Some(body),
            )
            .await?;
        let message = &value["data"];
        let id = message["message_id"]
            .as_str()
            .filter(|id| safe_id(id) && id.starts_with("om_"))
            .ok_or(Error::Unconfirmed)?;
        if id == reply.message_id || message["chat_id"].as_str() != Some(&reply.chat_id) {
            return Err(Error::Unconfirmed);
        }
        let actual_thread = match message.get("thread_id") {
            None => None,
            Some(Value::String(id)) => (!id.is_empty()).then_some(id.as_str()),
            _ => return Err(Error::Unconfirmed),
        };
        if actual_thread != reply.thread_id.as_deref() {
            return Err(Error::Unconfirmed);
        }
        Ok(id.into())
    }
}
