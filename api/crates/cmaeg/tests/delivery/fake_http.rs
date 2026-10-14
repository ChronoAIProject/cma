//! An in-process fake of the relay and Telegram client transports.
//!
//! It records every request and answers from a script, then from a default
//! responder. Latency is a `tokio::time::sleep` bounded by the request's
//! `timeout`, so under the paused clock a fake times out at exactly the
//! offset the real client would, and no socket is ever opened.
use cmaeg::{
    delivery::relay_client::RelayTransport,
    net::exchange::{HttpReply, TransportError},
    telegram::TelegramTransport,
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Mutex, time::Duration};
use tokio::time::Instant;

/// One request the fake received.
#[derive(Debug, Clone)]
pub struct Sent {
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub timeout: Duration,
    pub at: Instant,
}
impl Sent {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|value| value.to_str().unwrap())
    }
}

/// A scripted answer, delivered after `delay`.
#[derive(Debug, Clone)]
pub struct Answer {
    pub reply: Result<HttpReply, TransportError>,
    pub delay: Duration,
}
impl Answer {
    pub fn status(status: u16) -> Self {
        Self::json(status, json!({}))
    }
    pub fn json(status: u16, body: Value) -> Self {
        Self {
            reply: Ok(HttpReply::new(
                StatusCode::from_u16(status).unwrap(),
                serde_json::to_vec(&body).unwrap(),
            )),
            delay: Duration::ZERO,
        }
    }
    pub fn retry_after(mut self, seconds: &str) -> Self {
        if let Ok(reply) = &mut self.reply {
            reply
                .headers
                .insert("retry-after", seconds.parse().unwrap());
        }
        self
    }
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

type Responder = Box<dyn Fn(&Sent, usize) -> Answer + Send + Sync>;

pub struct FakeHttp {
    sent: Mutex<Vec<Sent>>,
    script: Mutex<VecDeque<Answer>>,
    responder: Responder,
}
impl FakeHttp {
    /// A fake answering like NyxID's relay: a fresh receipt for each reply.
    pub fn relay() -> Self {
        Self::new(|_, count| relay_answer(count))
    }
    /// A fake answering relay requests like NyxID and every other request
    /// like the Bot API, so one ordered log covers both platforms.
    pub fn platforms() -> Self {
        Self::new(|sent, count| {
            if sent.url.path().starts_with("/api/v1/channel-relay/") {
                relay_answer(count)
            } else {
                telegram_answer(sent, count)
            }
        })
    }
    pub fn new(responder: impl Fn(&Sent, usize) -> Answer + Send + Sync + 'static) -> Self {
        Self {
            sent: Mutex::default(),
            script: Mutex::default(),
            responder: Box::new(responder),
        }
    }
    /// Answer the next requests with `answers`, in order, before the default.
    pub fn script(self, answers: impl IntoIterator<Item = Answer>) -> Self {
        self.script.lock().unwrap().extend(answers);
        self
    }
    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
    async fn exchange(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<HttpReply, TransportError> {
        let sent = Sent {
            url,
            headers,
            body,
            timeout,
            at: Instant::now(),
        };
        let count = {
            let mut all = self.sent.lock().unwrap();
            all.push(sent.clone());
            all.len()
        };
        let scripted = self.script.lock().unwrap().pop_front();
        let answer = scripted.unwrap_or_else(|| (self.responder)(&sent, count));
        tokio::time::timeout(timeout, async {
            tokio::time::sleep(answer.delay).await;
            answer.reply
        })
        .await
        .unwrap_or(Err(TransportError::Unavailable))
    }
}
fn relay_answer(count: usize) -> Answer {
    Answer::json(
        200,
        json!({"message_id":format!("reply-{count}"),"platform_message_id":format!("platform-{count}")}),
    )
}
/// Each `sendMessage` gets the next message id in the requested chat, an edit
/// keeps its message id, and every other method succeeds.
fn telegram_answer(sent: &Sent, count: usize) -> Answer {
    let body = sent.json();
    let result = if sent.url.path().ends_with("/sendMessage")
        || sent.url.path().ends_with("/editMessageText")
    {
        let id = body["message_id"]
            .as_str()
            .and_then(|id| id.parse::<i64>().ok())
            .unwrap_or(100 + count as i64);
        let chat = body["chat_id"].as_str().unwrap().parse::<i64>().unwrap();
        json!({"message_id":id,"chat":{"id":chat}})
    } else {
        json!(true)
    };
    Answer::json(200, json!({"ok":true,"result":result}))
}
impl RelayTransport for FakeHttp {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(self.exchange(url, headers, body, timeout))
    }
}
impl TelegramTransport for FakeHttp {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(self.exchange(url, headers, body, timeout))
    }
}
