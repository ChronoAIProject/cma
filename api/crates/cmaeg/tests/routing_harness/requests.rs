use super::{
    app::App,
    relay_support::{now, now_rfc3339, Envelope},
};
use cmaeg::{auth::webhook, channels::resolver::ResolvedChannel};
use cmaeg_core::channel::Secret;
use serde_json::{json, Value};

impl App {
    pub async fn callback(&self, text: &str, id: &str) -> reqwest::Response {
        self.callback_to(&self.channel, text, id).await
    }
    pub async fn callback_to(
        &self,
        channel: &ResolvedChannel,
        text: &str,
        id: &str,
    ) -> reqwest::Response {
        let mut envelope = Envelope::new();
        envelope.payload["message_id"] = id.into();
        envelope.callback["message_id"] = id.into();
        envelope.reply["inbound_message_id"] = id.into();
        envelope.payload["content"]["text"] = text.into();
        envelope.payload["raw_platform_data"]["message"]["text"] = text.into();
        let (body, token) = envelope.signed();
        self.client
            .post(format!("{}/v1/callbacks/{}", self.base, channel.id))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", token)
            .header("x-nyxid-platform", "telegram")
            .header("x-nyxid-message-id", id)
            .header("x-nyxid-timestamp", now_rfc3339())
            .body(body)
            .send()
            .await
            .unwrap()
    }
    pub async fn native(&self, text: &str, id: u64) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/telegram/{}", self.base, self.channel.id))
            .header("x-telegram-bot-api-secret-token", "fixture-webhook")
            .json(
                &json!({"update_id":id,"message":{"message_id":id,"date":now()-1,
                "chat":{"id":42,"type":"private"},"from":{"id":7,"is_bot":false},"text":text}}),
            )
            .send()
            .await
            .unwrap()
    }
    pub async fn event(
        &self,
        channel: &ResolvedChannel,
        event: Value,
        stream: bool,
    ) -> reqwest::Response {
        self.event_mode(channel, event, stream.then_some("stream"))
            .await
    }
    pub async fn event_mode(
        &self,
        channel: &ResolvedChannel,
        event: Value,
        mode: Option<&str>,
    ) -> reqwest::Response {
        let body = event.to_string();
        let timestamp = now().to_string();
        let secret = Secret::new("Zml4dHVyZQ==");
        let signature = webhook::signature(&secret, "delivery-1", &timestamp, body.as_bytes());
        let suffix = mode.map_or_else(String::new, |mode| format!("?mode={mode}"));
        self.client
            .post(format!(
                "{}/v1/channels/{}/events{suffix}",
                self.base, channel.id
            ))
            .header("content-type", "application/cloudevents+json")
            .header("webhook-id", "delivery-1")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(body)
            .send()
            .await
            .unwrap()
    }
}
pub fn event(id: &str, targets: Option<&str>) -> Value {
    let mut event = json!({"specversion":"1.0","id":id,"source":"https://producer.example",
        "type":"build.done","data":{"text":"private payload"}});
    if let Some(targets) = targets {
        event["cmaegtargets"] = targets.into();
    }
    event
}
