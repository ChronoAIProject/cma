//! Local provider/record fixtures for native issuance and durable callback tests.
use super::{Destination, InteractionStore, PreparedCard};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        routing_plan::fixture,
        routing_store::test_server::{Row, Server},
        whatsapp::{self, VerifiedWhatsappMessage},
    },
    providers::EventContextSnapshot,
    turns::routed::adapter_test_server::HttpServer,
    whatsapp::{
        tests::{credentials, envelope, source, text},
        webhook,
    },
};
use cmaeg_core::{
    ids::{ConsumerId, ConversationId},
    message::OutboundMessage,
    seal::{EventRef, EventRefPayload},
};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct Fixture {
    pub server: Server,
    pub provider: HttpServer,
    pub store: Arc<InteractionStore>,
    pub channel: Arc<ResolvedChannel>,
    pub event: EventRefPayload,
    pub context: EventContextSnapshot,
}
pub(crate) fn message(list: bool) -> OutboundMessage {
    serde_json::from_value(json!({"text":"Choose", "blocks":[if list {
        json!({"type":"choice","id":"scope","prompt":"Select one","multi":false,"options":[{"id":"private-read","label":"Read"},{"id":"private-write","label":"Write"}]})
    } else {
        json!({"type":"buttons","id":"approval","options":[{"id":"private-yes","label":"Yes"},{"id":"private-no","label":"No"}]})
    }]})).unwrap()
}
pub(crate) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
impl Fixture {
    pub async fn new(named: bool) -> Self {
        let server = Server::start_realtime().await;
        let provider = HttpServer::start().await;
        let mut channel = fixture::request(|value| {
            fixture::targets(value);
            value["sources"] = json!([source()]);
            let c = credentials();
            value["secrets"]["whatsapp"] = json!({"app_secret":c.app_secret.as_str(),"access_token":c.access_token.as_str(),"verify_token":c.verify_token.as_str()});
        }, fixture::message("unused")).channel;
        let current = Arc::make_mut(&mut channel);
        current.definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        current.pointer.record_id = current.definition.record_id.clone();
        current.pointer.record_url = server
            .records
            .record_url(current.definition.record_id.as_str())
            .unwrap()
            .to_string();
        current.id = current.pointer.seal_id(&fixture::keys()).unwrap();
        let store = Arc::new(InteractionStore::new(
            server.records.clone(),
            Arc::new(fixture::keys()),
            provider.client(true, Duration::from_secs(2)),
        ));
        let raw = text("wamid.original", "Hello");
        let body = envelope(vec![raw]);
        let verified = VerifiedWhatsappMessage {
            channel: channel.clone(),
            source: source(),
            message: webhook::extract(&source(), &body).unwrap().remove(0),
            body,
            received: Instant::now(),
            received_at: now(),
        };
        let request = whatsapp::launch(&verified, None, &fixture::keys(), now()).unwrap();
        let mut event = request
            .event_ref
            .open(&fixture::keys(), now())
            .unwrap()
            .payload;
        if named {
            let consumer = ConsumerId::parse(fixture::REVIEWER).unwrap();
            event.conversation_id = ConversationId::derive_for_consumer(
                &fixture::keys(),
                &channel.id,
                channel.definition.partition.version,
                &consumer,
                &event.partition_key,
            );
            event.consumer = Some(consumer);
        }
        let context = EventContextSnapshot {
            schema_version: 1,
            channel_id: channel.id.clone(),
            conversation_id: event.conversation_id.clone(),
            activity: request.activity,
            event_ref: EventRef::seal(&event, &fixture::keys()).unwrap(),
        };
        provider.memory.lock().unwrap().config.event_context =
            Some(serde_json::to_value(&context).unwrap());
        let value = Self {
            server,
            provider,
            store,
            channel,
            event,
            context,
        };
        value.save_channel();
        value
    }
    pub fn save_channel(&self) {
        self.server.memory.channels.lock().unwrap().insert(
            self.channel.definition.record_id.to_string(),
            Row {
                owner: self.channel.definition.owner.subject.clone(),
                blob: cmaeg_core::seal::seal_blob(
                    &fixture::keys(),
                    &serde_json::to_vec(&self.channel.definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                expiry: i64::MAX,
                version: 1,
            },
        );
    }
    pub async fn card(&self, list: bool, bound: bool) -> PreparedCard {
        let mut card = self
            .store
            .prepare_with_context(
                &self.channel,
                &self.event,
                &message(list),
                self.context.clone(),
            )
            .await
            .unwrap();
        if bound {
            self.store
                .bind(
                    &mut card,
                    Destination {
                        outbound_id: "wamid.confirmed".into(),
                        platform_id: "wamid.confirmed".into(),
                    },
                )
                .await
                .unwrap();
        }
        card
    }
    pub fn locator(card: &PreparedCard, list: bool) -> String {
        let value = if list {
            &card.markup["action"]["sections"][0]["rows"][0]["id"]
        } else {
            &card.markup["action"]["buttons"][0]["reply"]["id"]
        };
        value.as_str().unwrap().into()
    }
    pub fn raw(&self, locator: &str, id: &str) -> Value {
        let kind = if locator.starts_with("cw1l:") {
            "list_reply"
        } else {
            "button_reply"
        };
        json!({"from":"447700900123","id":id,"timestamp":now().to_string(),"type":"interactive","context":{"id":"wamid.confirmed","from":"15551234567"},"interactive":{"type":kind,kind:{"id":locator,"title":"Untrusted changed title"}}})
    }
    pub fn verified(&self, raw: Value, extras: Vec<Value>) -> VerifiedWhatsappMessage {
        let mut batch = vec![raw];
        batch.extend(extras);
        let body = envelope(batch);
        VerifiedWhatsappMessage {
            channel: self.channel.clone(),
            source: source(),
            message: webhook::extract(&source(), &body).unwrap().remove(0),
            body,
            received: Instant::now(),
            received_at: now(),
        }
    }
    pub fn activity(&self, locator: &str, id: &str) -> cmaeg_core::activity::Activity {
        whatsapp::launch(
            &self.verified(self.raw(locator, id), vec![]),
            None,
            &fixture::keys(),
            now(),
        )
        .unwrap()
        .activity
    }
}
