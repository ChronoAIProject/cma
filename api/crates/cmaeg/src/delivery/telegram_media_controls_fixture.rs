use super::{
    gate::Ungated,
    intents::IntentReceipt,
    pacing::Pacing,
    relay_client::RelayClient,
    send::{SendContext, Sender},
};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        routing_plan::fixture,
        routing_store::test_server::{Row, Server},
    },
    interactions::{InteractionStore, PreparedCard},
    media::outbound,
    providers::EventContextSnapshot,
    telegram::media_fixture,
    turns::{observer::NoopObserver, routed::adapter_test_server::HttpServer},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    activity::Activity,
    ids::{ConsumerId, ConversationId},
    message::OutboundMessage,
    seal::{EventRef, EventRefPayload},
};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

pub(super) struct Fixture {
    pub records: Server,
    pub provider: HttpServer,
    pub platform: media_fixture::Fixture,
    pub store: Arc<InteractionStore>,
    pub sender: Sender,
    pub channel: Arc<ResolvedChannel>,
    pub event: EventRefPayload,
    pub activity: Activity,
}
pub(super) fn message(choice: bool, bytes: usize) -> OutboundMessage {
    serde_json::from_value(json!({"text":"Inspect this file", "native_attachments":[{
        "kind":"file","source":{"type":"base64","data":STANDARD.encode(vec![7;bytes])},
        "filename":"report.bin","mime_type":"application/octet-stream","caption":"File caption"
    }],"blocks":[if choice {
        json!({"type":"choice","id":"choice","prompt":"Pick one","multi":false,"options":[{"id":"accept","label":"Accept"},{"id":"reject","label":"Reject"}]})
    } else {
        json!({"type":"buttons","id":"action","options":[{"id":"accept","label":"Accept"},{"id":"reject","label":"Reject"}]})
    }]})).unwrap()
}
impl Fixture {
    pub async fn new(named: bool) -> Self {
        let records = Server::start_realtime().await;
        let provider = HttpServer::start().await;
        let platform = media_fixture::Fixture::new().await;
        platform.memory.echo_controls.store(true, Ordering::SeqCst);
        let mut activity = fixture::message("Review a file");
        activity.actor.id = "7".into();
        activity.conversation.id = "-42".into();
        activity.conversation.kind = cmaeg_core::activity::ConversationKind::Group;
        activity.conversation.thread_id = Some("7".into());
        let request = fixture::request(
            |value| {
                fixture::targets(value);
                value["sources"] =
                    json!([{"type":"telegram","bot_id":"77","admission":{"type":"all"}}]);
                value["secrets"]["telegram_bot_token"] = "77:fixture".into();
            },
            activity.clone(),
        );
        let mut event = request
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        let mut channel = request.channel;
        let current = Arc::make_mut(&mut channel);
        current.definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        current.pointer.record_id = current.definition.record_id.clone();
        current.pointer.record_url = records
            .records
            .record_url(current.definition.record_id.as_str())
            .unwrap()
            .to_string();
        current.id = current.pointer.seal_id(&fixture::keys()).unwrap();
        event.channel_id = channel.id.clone();
        event.record_id = channel.definition.record_id.clone();
        if let cmaeg_core::seal::ReplyAddress::Telegram { thread_id, .. } = &mut event.reply {
            *thread_id = activity.conversation.thread_id.clone();
        }
        event.consumer = named.then(|| ConsumerId::parse(fixture::REVIEWER).unwrap());
        event.conversation_id = match &event.consumer {
            Some(id) => ConversationId::derive_for_consumer(
                &fixture::keys(),
                &channel.id,
                1,
                id,
                &event.partition_key,
            ),
            None => ConversationId::derive(&fixture::keys(), &channel.id, 1, &event.partition_key),
        };
        event.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + 3600;
        let context = EventContextSnapshot {
            schema_version: 1,
            channel_id: channel.id.clone(),
            conversation_id: event.conversation_id.clone(),
            activity: activity.clone(),
            event_ref: EventRef::seal(&event, &fixture::keys()).unwrap(),
        };
        provider.memory.lock().unwrap().config.event_context =
            Some(serde_json::to_value(context).unwrap());
        let store = Arc::new(InteractionStore::new(
            records.records.clone(),
            Arc::new(fixture::keys()),
            provider.client(true, Duration::from_secs(2)),
        ));
        let mut relay = RelayClient::new("https://nyx.example", Duration::from_secs(2))
            .unwrap()
            .with_telegram_media_controls(true)
            .with_consumer_interactions(true)
            .with_interactions(store.clone());
        relay.telegram = platform.api.clone();
        let sender = Sender {
            custom: super::custom::CustomDelivery::new(
                "https://gateway.example".into(),
                Arc::default(),
            ),
            keys: Arc::new(fixture::keys()),
            relay,
            pacing: Pacing::default(),
            consumed: Mutex::default(),
            observer: Arc::new(NoopObserver),
        };
        let fixture = Self {
            records,
            provider,
            platform,
            store,
            sender,
            channel,
            event,
            activity,
        };
        fixture.save_channel();
        fixture
    }
    pub fn save_channel(&self) {
        self.records.memory.channels.lock().unwrap().insert(
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
    pub async fn send(&self, message: &OutboundMessage) -> IntentReceipt {
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &self.channel,
            first_send_done: false,
            gate: &Ungated,
        };
        self.sender.reply(&ctx, &self.event, message).await.0
    }
    pub async fn prepared(&self) -> PreparedCard {
        let message = message(false, 4);
        let file = outbound::prepare(&message.native_attachments)
            .await
            .unwrap()
            .remove(0);
        self.store
            .prepare_telegram_media(&self.channel, &self.event, &message, &file)
            .await
            .unwrap()
    }
    pub fn locator(&self) -> String {
        let calls = self.platform.calls();
        let markup: serde_json::Value =
            serde_json::from_str(&media_fixture::field(&calls[0], "reply_markup")).unwrap();
        markup["inline_keyboard"][0][0]["callback_data"]
            .as_str()
            .unwrap()
            .into()
    }
}
