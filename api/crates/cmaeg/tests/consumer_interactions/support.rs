//! Signed callbacks and durable records with a real consumer-scoped provider context.
use super::super::{
    consumer_support::{add_reviewer, issue, reviewer, reviewer_key},
    fake_cma::FakeCma,
    relay_support::{now, Envelope},
    support::App,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    http,
    interactions::{Destination, InteractionStore, PreparedCard},
    records::RecordClient,
};
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{seal_blob, EventRef},
};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct Cards {
    pub app: App,
    pub records: FakeCma,
    pub store: Arc<InteractionStore>,
    pub named: EventRef,
    pub platform: String,
}
impl Cards {
    pub async fn new(platform: &str) -> Self {
        let app = App::on_private(platform).await;
        let records = FakeCma::start(false).await;
        let store = Arc::new(InteractionStore::new(
            RecordClient::new(&records.base_url, "cma-service").unwrap(),
            app.state.keys.clone(),
            app.state.providers.clone(),
        ));
        let mut cards = Self {
            named: app.event_ref.clone(),
            app,
            records,
            store,
            platform: platform.into(),
        };
        cards.change(add_reviewer).await;
        cards.named = issue(&cards.app, &reviewer(), &reviewer_key(), None).await;
        cards
    }
    pub async fn channel(&self) -> Arc<ResolvedChannel> {
        self.app
            .state
            .resolver
            .resolve(self.app.fixture.id.as_str())
            .await
            .unwrap()
    }
    pub async fn change(&self, edit: impl FnOnce(&mut Value)) {
        let mut value = self.channel().await.definition.to_blob_json();
        edit(&mut value);
        let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
        definition.validate().unwrap();
        *self.app.fixture.backend.blob.lock().unwrap() = seal_blob(
            &self.app.state.keys,
            definition.to_blob_json().to_string().as_bytes(),
        )
        .unwrap();
        self.app
            .state
            .resolver
            .invalidate(self.app.fixture.id.as_str());
        self.app
            .fixture
            .state
            .resolver
            .invalidate(self.app.fixture.id.as_str());
    }
    pub async fn card(&self, named: bool) -> PreparedCard {
        let reference = if named {
            &self.named
        } else {
            &self.app.event_ref
        };
        let event = reference.open(&self.app.state.keys, now()).unwrap().payload;
        let message = serde_json::from_value(json!({"text":"Choose","blocks":[
            {"type":"buttons","id":"decision","options":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}]}]})).unwrap();
        let mut card = self
            .store
            .prepare(self.channel().await.as_ref(), &event, &message)
            .await
            .unwrap();
        self.store
            .bind(
                &mut card,
                Destination {
                    outbound_id: "outbound-1".into(),
                    platform_id: self.message_id().into(),
                },
            )
            .await
            .unwrap();
        card
    }
    pub fn restart(&mut self) {
        let pointer = self
            .app
            .fixture
            .id
            .open(&self.app.state.keys)
            .unwrap()
            .payload;
        let origin = pointer.record_url.split("/api/").next().unwrap();
        let resolver = cmaeg::channels::resolver::ChannelResolver::new(
            Arc::new(RecordClient::new(origin, "test-service-bearer").unwrap()),
            self.app.state.keys.clone(),
            Default::default(),
        );
        self.app.state.resolver = resolver.clone();
        self.app.fixture.state.resolver = resolver;
        self.store = Arc::new(InteractionStore::new(
            RecordClient::new(&self.records.base_url, "cma-service").unwrap(),
            self.app.state.keys.clone(),
            self.app.state.providers.clone(),
        ));
    }
    pub fn message_id(&self) -> &str {
        match self.platform.as_str() {
            "whatsapp" => "wamid.card",
            "telegram" => "42",
            _ => "om_card",
        }
    }
    pub fn click(&self, card: &PreparedCard) -> Envelope {
        let mut e = Envelope::on(&self.platform);
        let locator = match self.platform.as_str() {
            "whatsapp" => &card.markup["action"]["buttons"][0]["reply"]["id"],
            "telegram" => &card.markup["inline_keyboard"][0][0]["callback_data"],
            _ => card.markup["body"]["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|v| v.pointer("/behaviors/0/value/cma_action"))
                .unwrap(),
        };
        let locator = locator.as_str().unwrap();
        e.payload["reply_to_platform_message_id"] = self.message_id().into();
        match self.platform.as_str() {
            "whatsapp" => {
                e.payload["conversation"]["platform_id"] = "7".into();
                e.payload["content"] = json!({"type":"text","text":"Yes"});
                e.payload["raw_platform_data"] = json!({"id":"wamid.click","from":"7","type":"interactive","context":{"id":"wamid.card"},"interactive":{"type":"button_reply","button_reply":{"id":locator,"title":"Yes"}}});
            }
            "telegram" => {
                e.payload["content"] = json!({"type":"interaction","text":"Yes"});
                e.payload["raw_platform_data"] = json!({"callback_query":{"id":"query-1","data":locator,"from":{"id":7,"is_bot":false},"message":{"message_id":42,"chat":{"id":42,"type":"private"}}}});
            }
            _ => {
                let value = json!({"cma_action":locator});
                e.payload["raw_platform_data"] = json!({"schema":"2.0","header":{"event_id":"click-1","event_type":"card.action.trigger"},"event":{"operator":{"open_id":"ou_sender"},"host":"im_message","action":{"tag":"button","value":value},"context":{"open_chat_id":"oc_chat","open_message_id":"om_card"}}});
                e.payload["content"] = json!({"type":"card_action","text":json!({"tag":"button","value":value,"form_value":null,"open_message_id":"om_card"}).to_string()});
            }
        }
        e
    }
    pub async fn send(&self, e: &Envelope) -> (u16, Value) {
        let (body, token) = e.signed();
        let response = http::callbacks::callback(
            State(
                self.app
                    .fixture
                    .state
                    .clone()
                    .with_interactions(self.store.clone()),
            ),
            Path(self.app.fixture.id.to_string()),
            self.app.fixture.local_request(Body::from(body), &token),
        )
        .await;
        let status = response.status().as_u16();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
}
