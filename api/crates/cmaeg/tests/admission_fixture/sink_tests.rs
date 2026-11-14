use super::sink_reply;
use crate::{bootstrap, ledger::Ledger, protocol::Event};
use axum::{extract::Request, http::StatusCode, routing::post, Router};
use cmaeg::{
    delivery::{
        intents::DeliveryOutcome,
        relay_client::{Credential, RelayClient},
    },
    ingress::routing_store::EventIdentity,
};
use cmaeg_core::{
    channel::{Platform, Secret},
    ids::{ChannelId, EventId},
    seal::ReplyAddress,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Sink {
    key: Secret,
    events: Mutex<BTreeMap<String, Event>>,
    ledger: Mutex<Ledger>,
}

struct Fixture {
    sink: Arc<Sink>,
    address: ReplyAddress,
    reply: Secret,
    other_reply: String,
    unsigned_event: String,
    base: String,
    task: tokio::task::JoinHandle<()>,
    stop: tokio::sync::oneshot::Sender<()>,
}

impl Fixture {
    async fn new() -> Self {
        let definition =
            bootstrap::channel("http://127.0.0.1:1", &bootstrap::random_secret().unwrap()).unwrap();
        let mut events = BTreeMap::new();
        for label in ["first", "other", "unsigned"] {
            let identity = EventIdentity {
                owner: definition.owner.subject.clone(),
                channel_id: ChannelId::new("cmaeg1.ch.sink-fixture").unwrap(),
                record_id: definition.record_id.clone(),
                event_id: EventId::new(uuid::Uuid::new_v4().to_string()).unwrap(),
            };
            let mut event = Event::new(identity);
            if label != "unsigned" {
                event.signed();
            }
            events.insert(label.into(), event);
        }
        let reply = Secret::new(events["first"].signed.as_ref().unwrap().reply.clone());
        let other_reply = events["other"].signed.as_ref().unwrap().reply.clone();
        let unsigned_event = events["unsigned"].identity.event_id.to_string();
        let address = ReplyAddress::NyxidRelay {
            platform: Platform::Telegram,
            message_id: events["first"].identity.event_id.to_string(),
            route_id: "route-1".into(),
            thread_reply: false,
            reply_token: reply.clone(),
            reply_token_expires_at: i64::MAX,
        };
        let sink = Arc::new(Sink {
            key: definition.secrets.channel_key.unwrap(),
            events: Mutex::new(events),
            ledger: Mutex::new(Ledger::default()),
        });
        let router = Router::new().route(
            "/api/v1/channel-relay/reply",
            post({
                let sink = sink.clone();
                move |request: Request| {
                    let sink = sink.clone();
                    async move { sink_reply(request, &sink.key, &sink.events, &sink.ledger).await }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            sink,
            address,
            reply,
            other_reply,
            unsigned_event,
            base,
            task,
            stop,
        }
    }

    fn client(&self) -> RelayClient {
        RelayClient::new(&self.base, Duration::from_secs(2)).unwrap()
    }

    fn message_id(&self) -> &str {
        let ReplyAddress::NyxidRelay { message_id, .. } = &self.address else {
            panic!("relay fixture")
        };
        message_id
    }

    async fn finish(self) {
        self.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn production_channel_key_delivery_is_confirmed_and_counted() {
    let f = Fixture::new().await;
    // DurableReply deliberately chooses ChannelKey; it cannot consume a
    // single-use ingress reply token when replaying retained provider output.
    let outcome = f
        .client()
        .send_rich(
            &f.address,
            Credential::ChannelKey(&f.sink.key),
            "fixture terminal text",
            None,
        )
        .await
        .0;
    assert!(
        matches!(outcome, DeliveryOutcome::Confirmed { .. }),
        "durable channel-key delivery must be confirmed"
    );
    {
        let ledger = f.sink.ledger.lock().unwrap();
        assert_eq!(ledger.effects.len(), 1);
        assert!(ledger.effects[0].text_matches);
        assert_eq!(
            ledger.effects[0].event_sha256,
            crate::ledger::hash(f.message_id().as_bytes())
        );
    }
    f.finish().await;
}

#[tokio::test]
async fn reply_tokens_remain_event_scoped_and_unknown_keys_or_events_are_rejected() {
    let f = Fixture::new().await;
    let outcome = f
        .client()
        .send_rich(
            &f.address,
            Credential::ReplyToken(&f.reply),
            "fixture terminal text",
            None,
        )
        .await
        .0;
    assert!(matches!(outcome, DeliveryOutcome::Confirmed { .. }));
    let client = reqwest::Client::new();
    let unknown = uuid::Uuid::new_v4().to_string();
    let wrong_key = bootstrap::random_secret().unwrap();
    for (credential, event) in [
        (wrong_key.as_str(), f.message_id()),
        (f.other_reply.as_str(), f.message_id()),
        (f.sink.key.as_str(), unknown.as_str()),
        (f.reply.as_str(), unknown.as_str()),
        (f.sink.key.as_str(), f.unsigned_event.as_str()),
    ] {
        let response = client
            .post(format!("{}/api/v1/channel-relay/reply", f.base))
            .bearer_auth(credential)
            .json(&json!({"message_id":event,"reply":{"text":"fixture terminal text"}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["code"],
            "sink_auth"
        );
        assert_eq!(f.sink.ledger.lock().unwrap().effects.len(), 1);
    }
    f.finish().await;
}

#[tokio::test]
async fn missing_authentication_and_malformed_replies_never_record_effects() {
    let f = Fixture::new().await;
    let client = reqwest::Client::new();
    let endpoint = format!("{}/api/v1/channel-relay/reply", f.base);
    let response = client
        .post(&endpoint)
        .json(&json!({"message_id":f.message_id(),"reply":{"text":"fixture terminal text"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for body in [
        json!({"reply":{"text":"fixture terminal text"}}),
        json!({"message_id":f.message_id(),"reply":{}}),
    ] {
        let response = client
            .post(&endpoint)
            .bearer_auth(f.reply.as_str())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(f.sink.ledger.lock().unwrap().effects.is_empty());
    f.finish().await;
}
