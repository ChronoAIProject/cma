//! Reuse the existing relay signature contract with per-run generated keys.
use super::{ledger::hash, signer::Signer};
use cmaeg::ingress::routing_store::EventIdentity;
use serde_json::{json, Value};

pub struct Event {
    pub identity: EventIdentity,
    pub signed: Option<Signed>,
}
#[derive(Clone)]
pub struct Signed {
    pub body: Vec<u8>,
    pub callback: String,
    pub reply: String,
    pub timestamp: String,
}
impl Event {
    pub fn new(identity: EventIdentity) -> Self {
        Self {
            identity,
            signed: None,
        }
    }
    pub fn signed(&mut self) -> Signed {
        self.signed
            .get_or_insert_with(|| sign(self.identity.event_id.as_str()))
            .clone()
    }
}
fn claims(audience: &str, kind: &str, ttl: i64, now: i64) -> Value {
    json!({"iss":"https://eg-diagnostic.invalid","aud":audience,"token_type":kind,"iat":now,"exp":now+ttl,"jti":uuid::Uuid::new_v4().to_string(),"api_key_id":"key-1","platform":"telegram"})
}
fn sign(event: &str) -> Signed {
    let now = time::OffsetDateTime::now_utc();
    let timestamp = now
        .format(&time::format_description::well_known::Rfc3339)
        .expect("fixed RFC3339 format");
    let mut reply = claims(
        "channel-relay/reply",
        "relay_reply",
        1800,
        now.unix_timestamp(),
    );
    reply["inbound_message_id"] = event.into();
    reply["conversation_id"] = "route-1".into();
    let reply = Signer::shared().token(&reply);
    let payload = json!({"message_id":event,"correlation_id":uuid::Uuid::new_v4().to_string(),"platform":"telegram","agent":{"api_key_id":"key-1","name":"Fixture"},"conversation":{"id":"route-1","platform_id":"42","type":"private"},"sender":{"platform_id":"7","display_name":"Fixture"},"content":{"type":"text","text":"fixture input"},"timestamp":timestamp,"reply_token":reply,
        "raw_platform_data":{"message":{"message_id":11,"chat":{"id":42,"type":"private"},"from":{"id":7,"is_bot":false},"text":"fixture input"}}});
    let body = payload.to_string().into_bytes();
    let mut callback = claims(
        "channel-relay/callback",
        "relay_callback",
        300,
        now.unix_timestamp(),
    );
    callback["jti"] = payload["correlation_id"].clone();
    callback["message_id"] = event.into();
    callback["body_sha256"] = hash(&body).into();
    Signed {
        body,
        callback: Signer::shared().token(&callback),
        reply,
        timestamp,
    }
}

pub fn snapshot(row: &super::ledger::ResponseRecord, status: &str) -> Value {
    json!({"id":row.response_id,"object":"response","status":status,"conversation":{"id":row.conversation},"metadata":row.metadata,"output":[]})
}
pub fn frames(row: &super::ledger::ResponseRecord, after: Option<u64>, admission: bool) -> String {
    let events = [
        json!({"type":"response.created","sequence_number":0,"response":snapshot(row,"queued")}),
        json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"fixture-text","delta":"fixture terminal text"}),
        json!({"type":"response.completed","sequence_number":2,"response":snapshot(row,"completed")}),
    ];
    events
        .iter()
        .take(if admission { 1 } else { 3 })
        .enumerate()
        .filter(|(ordinal, _)| after.is_none_or(|after| *ordinal as u64 > after))
        .map(|(ordinal, event)| format!("id: {ordinal}\ndata: {event}\n\n"))
        .collect()
}
