//! Native-only records and provider snapshots; every transport stays local.
use super::super::{InteractionStore, PreparedCard};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        routing_plan::fixture as routed,
        routing_store::{
            test_server::{Row, Server},
            EventIdentity,
        },
    },
    native_lark::{
        callback::Callback,
        message,
        receipts::{Receipt, Store, RETENTION},
        test_support as native,
    },
    providers::EventContextSnapshot,
    turns::routed::adapter_test_server::HttpServer,
};
use cmaeg_core::{
    channel::{LarkRegion, Source},
    ids::{ConsumerId, ConversationId},
    message::OutboundMessage,
    seal::{EventRef, EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
pub(crate) struct Fixture {
    pub server: Server,
    _provider: HttpServer,
    pub store: Arc<InteractionStore>,
    pub channel: Arc<ResolvedChannel>,
    pub event: EventRefPayload,
    pub context: EventContextSnapshot,
}
pub(crate) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub(crate) fn outbound(form: bool) -> OutboundMessage {
    serde_json::from_value(if form {
        json!({"blocks":[{"type":"form","id":"request","fields":[
            {"id":"reason","label":"Reason","required":true,"kind":{"type":"text"}},
            {"id":"amount","label":"Amount","required":false,"kind":{"type":"number"}},
            {"id":"day","label":"Day","required":false,"kind":{"type":"date"}},
            {"id":"scope","label":"Scope","required":false,"kind":{"type":"choice","options":[{"id":"read","label":"Read"},{"id":"write","label":"Write"}]}}
        ]}]})
    } else {
        json!({"text":"Choose","blocks":[{"type":"buttons","id":"approval","options":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}]}]})
    }).unwrap()
}
impl Fixture {
    pub async fn new(named: bool, thread: bool, region: LarkRegion) -> Self {
        Self::with_group(named, thread, thread, region).await
    }
    pub async fn new_group(named: bool, region: LarkRegion) -> Self {
        Self::with_group(named, false, true, region).await
    }
    async fn with_group(named: bool, thread: bool, group: bool, region: LarkRegion) -> Self {
        let server = Server::start_realtime().await;
        let provider = HttpServer::start().await;
        let Source::NativeLark(mut source) = native::source() else {
            panic!()
        };
        source.identity.region = region;
        let mut channel = routed::request(|v| {
            routed::targets(v);
            v["sources"] = json!([Source::NativeLark(source.clone())]);
            let c = native::credentials();
            v["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
        }, routed::message("unused")).channel;
        let current = Arc::make_mut(&mut channel);
        current.definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        current.pointer.record_id = current.definition.record_id.clone();
        current.pointer.record_url = server
            .records
            .record_url(current.definition.record_id.as_str())
            .unwrap()
            .to_string();
        current.id = current.pointer.seal_id(&routed::keys()).unwrap();
        let mut raw = native::text("om_original", "hello", now());
        if thread {
            raw["event"]["message"]["thread_id"] = "omt_original".into();
        }
        if group {
            raw["event"]["message"]["chat_type"] = "group".into();
        }
        let msg = message::extract(&source, &raw, now()).unwrap().unwrap();
        let request = message::launch(
            channel.clone(),
            &source,
            &msg,
            None,
            &routed::keys(),
            Instant::now(),
            now(),
        )
        .unwrap();
        let mut event = request
            .event_ref
            .open(&routed::keys(), now())
            .unwrap()
            .payload;
        if named {
            let consumer = ConsumerId::parse(routed::REVIEWER).unwrap();
            event.conversation_id = ConversationId::derive_for_consumer(
                &routed::keys(),
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
            event_ref: EventRef::seal(&event, &routed::keys()).unwrap(),
        };
        provider.memory.lock().unwrap().config.event_context =
            Some(serde_json::to_value(&context).unwrap());
        let store = Arc::new(InteractionStore::new(
            server.records.clone(),
            Arc::new(routed::keys()),
            provider.client(true, Duration::from_secs(2)),
        ));
        let mut f = Self {
            server,
            _provider: provider,
            store,
            channel,
            event,
            context,
        };
        f.save_channel();
        // Exercise the production restore path, including the exact default and
        // named consumer key hashes. The planner fixture's placeholder hash is
        // deliberately not valid retained custody after a fresh restore.
        f.channel = crate::channels::resolver::ChannelResolver::new(
            Arc::new(f.server.records.clone()),
            Arc::new(routed::keys()),
            Default::default(),
        )
        .resolve_current(f.channel.id.as_str())
        .await
        .unwrap();
        f
    }
    pub fn save_channel(&self) {
        self.server.memory.channels.lock().unwrap().insert(
            self.channel.definition.record_id.to_string(),
            Row {
                owner: self.channel.definition.owner.subject.clone(),
                blob: cmaeg_core::seal::seal_blob(
                    &routed::keys(),
                    &serde_json::to_vec(&self.channel.definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                expiry: i64::MAX,
                version: 1,
            },
        );
    }
    pub fn receipt(&self) -> Receipt {
        let ReplyAddress::NativeLark(mut reply) = self.event.reply.clone() else {
            panic!()
        };
        reply.message_id = "om_confirmed".into();
        let consumer = self.channel.consumer(self.event.consumer.as_ref()).unwrap();
        Receipt {
            kind: "native_lark_receipt_v1".into(),
            original: EventIdentity {
                owner: self.channel.definition.owner.subject.clone(),
                channel_id: self.channel.id.clone(),
                record_id: self.channel.pointer.record_id.clone(),
                event_id: self.event.event_id.clone(),
            },
            source: self.context.activity.source.clone(),
            recipient: reply.chat_id.clone(),
            message_id: reply.message_id.clone(),
            reply,
            consumer: consumer.id.clone(),
            provider: consumer.provider.clone(),
            partition_key: self.event.partition_key.clone(),
            conversation_id: self.event.conversation_id.clone(),
            confirmed_at: now(),
            retained_until: now() + RETENTION,
        }
    }
    pub async fn card(&self, form: bool, bound: bool) -> PreparedCard {
        let mut message = outbound(form);
        message.thread_reply = self.context.activity.conversation.thread_id.is_some();
        let mut card = self
            .store
            .prepare_native_lark(&self.channel, &self.event, &message, form)
            .await
            .unwrap();
        if bound {
            let receipt = self.receipt();
            Store::new(self.server.records.clone(), Arc::new(routed::keys()))
                .put(&receipt)
                .await
                .unwrap();
            self.store
                .bind_native_lark(&mut card, receipt)
                .await
                .unwrap();
        }
        card
    }
    pub fn callback(&self, card: &PreparedCard, event: &str, index: usize) -> Callback {
        let form = card.card.native_lark.as_ref().unwrap().form;
        Callback {
            event_id: event.into(),
            identity: card
                .card
                .native_lark
                .as_ref()
                .unwrap()
                .reply()
                .identity
                .clone(),
            actor: self.context.activity.actor.id.clone(),
            chat: self.context.activity.conversation.id.clone(),
            message: "om_confirmed".into(),
            locator: format!(
                "{}:{}:{}:{index}",
                if form { "cnlf1" } else { "cnl1" },
                uuid::Uuid::parse_str(&card.card.id).unwrap().simple(),
                card.card.revision
            ),
            form: form.then(|| {
                json!({"f0":"Approved","f1":"12.5","f2":"2024-02-29 +0800","f3":"1"})
                    .as_object()
                    .unwrap()
                    .clone()
            }),
            occurred_at: now(),
        }
    }
    pub fn raw(&self, callback: &Callback) -> Value {
        let mut action = json!({"tag":"button","value":{"cma_action":callback.locator}});
        if let Some(form) = &callback.form {
            action["name"] = "submit".into();
            action["form_value"] = json!(form);
        }
        let mut raw = native::envelope(
            "card.action.trigger",
            &callback.event_id,
            json!({"host":"im_message","operator":{"tenant_key":callback.identity.tenant_key,"open_id":callback.actor},"context":{"open_chat_id":callback.chat,"open_message_id":callback.message},"action":action,"token":"fixture-card-update-credential"}),
        );
        raw["header"]["create_time"] = (callback.occurred_at * 1_000_000).to_string().into();
        raw
    }
}
