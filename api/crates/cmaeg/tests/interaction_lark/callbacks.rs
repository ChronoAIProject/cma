//! Signed callbacks traverse the real verifier, current ACL and durable CAS.
use super::{
    fake_cma::FakeCma,
    relay_support::{now, Envelope},
    support::App,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::StatusCode,
};
use cmaeg::{
    auth::AgentCaller,
    http,
    interactions::{Destination, InteractionStore, PreparedCard},
    records::RecordClient,
};
use cmaeg_core::{
    activity::{ActivityKind, ConversationKind, SourceKind},
    channel::Platform,
    message::OutboundMessage,
};
use serde_json::{json, Value};
use std::sync::Arc;

struct Cards {
    app: App,
    _records: FakeCma,
    store: Arc<InteractionStore>,
}
impl Cards {
    async fn new(platform: &str, private: bool) -> Self {
        let app = if private {
            App::on_private(platform).await
        } else {
            App::on(platform).await
        };
        let records = FakeCma::start(false).await;
        let store = Arc::new(InteractionStore::new(
            RecordClient::new(&records.base_url, "cma-service").unwrap(),
            app.state.keys.clone(),
            app.state.providers.clone(),
        ));
        Self {
            app,
            _records: records,
            store,
        }
    }
    async fn prepare(&self) -> PreparedCard {
        let event = self
            .app
            .event_ref
            .open(&self.app.state.keys, now())
            .unwrap()
            .payload;
        let channel = self
            .app
            .state
            .resolver
            .resolve(event.channel_id.as_str())
            .await
            .unwrap();
        let message: OutboundMessage = serde_json::from_value(json!({"text":"Choose","blocks":[
            {"type":"choice","id":"services","prompt":"Services","multi":true,"options":[
                {"id":"a","label":"A","selected":true},{"id":"b","label":"B"}]}]}))
        .unwrap();
        self.store
            .prepare(&channel, &event, &message)
            .await
            .unwrap()
    }
    async fn bind(&self, card: &mut PreparedCard) {
        self.store
            .bind(
                card,
                Destination {
                    outbound_id: "outbound-1".into(),
                    platform_id: "om_card".into(),
                },
            )
            .await
            .unwrap();
    }
    async fn groups(&self, groups: &str) {
        let channel = self
            .app
            .state
            .resolver
            .resolve(self.app.fixture.id.as_str())
            .await
            .unwrap();
        let mut definition = channel.definition.clone();
        let mut source = serde_json::to_value(&definition.sources[0]).unwrap();
        source["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":groups});
        definition.sources[0] = serde_json::from_value(source).unwrap();
        definition.validate().unwrap();
        *self.app.fixture.backend.blob.lock().unwrap() = cmaeg_core::seal::seal_blob(
            &self.app.state.keys,
            definition.to_blob_json().to_string().as_bytes(),
        )
        .unwrap();
        self.app
            .state
            .resolver
            .invalidate(self.app.fixture.id.as_str());
    }
    async fn send(&self, envelope: &Envelope) -> (StatusCode, Value) {
        let (body, token) = envelope.signed();
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
        let status = response.status();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
}
fn data(card: &PreparedCard, index: usize) -> String {
    card.markup["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.pointer("/behaviors/0/value/cma_action"))
        .nth(index)
        .unwrap()
        .as_str()
        .unwrap()
        .into()
}
fn callback(platform: &str, data: &str, id: &str) -> Envelope {
    let mut envelope = Envelope::on(platform);
    envelope.payload["conversation"]["type"] = "group".into();
    envelope.payload["reply_to_platform_message_id"] = "om_card".into();
    let value = json!({"cma_action":data});
    envelope.payload["raw_platform_data"] = json!({"schema":"2.0","header":{
        "event_id":id,"event_type":"card.action.trigger"},"event":{
            "operator":{"open_id":"ou_sender"},"host":"im_message",
            "action":{"tag":"button","value":value},
            "context":{"open_chat_id":"oc_chat","open_message_id":"om_card"}}});
    sync_text(&mut envelope);
    envelope
}
fn sync_text(envelope: &mut Envelope) {
    let event = &envelope.payload["raw_platform_data"]["event"];
    let normalized = json!({"tag":event["action"]["tag"],"value":event["action"]["value"],
        "form_value":event["action"]["form_value"],"open_message_id":event["context"]["open_message_id"]});
    envelope.payload["content"] = json!({"type":"card_action","text":normalized.to_string()});
}

#[tokio::test]
async fn signed_lark_and_feishu_clicks_survive_restart_and_select_only_stored_values() {
    for platform in ["lark", "feishu"] {
        let mut cards = Cards::new(platform, false).await;
        let mut prepared = cards.prepare().await;
        assert_eq!(prepared.markup["schema"], "2.0");
        let click = callback(platform, &data(&prepared, 1), "event-click");
        assert_eq!(
            cards.send(&click).await.1["reason"],
            "interaction_expired_or_invalid"
        );
        cards.bind(&mut prepared).await;
        cards.store = Arc::new(InteractionStore::new(
            RecordClient::new(&cards._records.base_url, "cma-service").unwrap(),
            cards.app.state.keys.clone(),
            cards.app.state.providers.clone(),
        ));
        let (status, first) = cards.send(&click).await;
        assert_eq!(status, 202);
        assert_eq!(first["status"], "accepted");
        let (_, replay) = cards.send(&click).await;
        assert_eq!(first, replay);
        {
            let requests = cards.app.fixture.recorder.requests.lock().unwrap();
            let ActivityKind::Interaction(result) = &requests.last().unwrap().activity.kind else {
                panic!("not interaction")
            };
            assert_eq!(result.block_id, "services");
            assert_eq!(result.action_id, "b");
            assert_eq!(result.values["selected"], json!(["a", "b"]));
            assert_eq!(result.values["revision"], 1);
        }
        assert_eq!(
            cards
                .send(&callback(platform, &data(&prepared, 1), "different-click"))
                .await
                .1["reason"],
            "interaction_expired_or_invalid"
        );
    }
}

#[tokio::test]
async fn lark_clicks_fence_raw_identity_record_scope_message_action_and_revision() {
    let cards = Cards::new("lark", false).await;
    let mut prepared = cards.prepare().await;
    cards.bind(&mut prepared).await;
    let token = data(&prepared, 0);
    let original = callback("lark", &token, "click-1");
    for field in ["actor", "chat", "message", "revision", "index", "text"] {
        let mut click = callback("lark", &token, "click-1");
        match field {
            "actor" => {
                click.payload["sender"]["platform_id"] = "ou_intruder".into();
                click.payload["raw_platform_data"]["event"]["operator"]["open_id"] =
                    "ou_intruder".into();
            }
            "chat" => {
                click.payload["conversation"]["platform_id"] = "oc_other".into();
                click.payload["raw_platform_data"]["event"]["context"]["open_chat_id"] =
                    "oc_other".into();
            }
            "message" => {
                click.payload["reply_to_platform_message_id"] = "om_other".into();
                click.payload["raw_platform_data"]["event"]["context"]["open_message_id"] =
                    "om_other".into();
            }
            "revision" => {
                click.payload["raw_platform_data"]["event"]["action"]["value"]["cma_action"] =
                    token.replace(":1:", ":2:").into()
            }
            "index" => {
                click.payload["raw_platform_data"]["event"]["action"]["value"]["cma_action"] =
                    format!("{}:999", token.rsplit_once(':').unwrap().0).into()
            }
            _ => {}
        }
        sync_text(&mut click);
        if field == "text" {
            click.payload["content"]["text"] = "forged".into();
        }
        let (status, body) = cards.send(&click).await;
        if field == "text" {
            assert_eq!(status, 401);
        } else {
            assert_eq!(
                body["reason"], "interaction_expired_or_invalid",
                "{field}: {body}"
            );
        }
        assert!(cards
            .store
            .load(&prepared.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
    }
    let channel = cards
        .app
        .state
        .resolver
        .resolve(cards.app.fixture.id.as_str())
        .await
        .unwrap();
    let mut wrong_platform = cards.app.activity.clone();
    wrong_platform.source = SourceKind::NyxidRelay {
        platform: Platform::Feishu,
        route_id: "route-1".into(),
    };
    assert!(cards
        .store
        .consume(
            &channel,
            &wrong_platform,
            "route-1",
            "click-1",
            &token,
            "om_card",
            &|c| c.is_none()
        )
        .await
        .is_err());
    assert_eq!(cards.send(&original).await.1["status"], "accepted");
    let reference = prepared
        .card
        .reference
        .as_ref()
        .unwrap()
        .open(&cards.app.state.keys)
        .unwrap()
        .payload;
    let mut replacement = cards
        .store
        .replacement(&channel, &reference, &prepared.card.message)
        .await
        .unwrap();
    assert_eq!(replacement.card.revision, 2);
    assert_eq!(
        cards.send(&original).await.1["reason"],
        "interaction_expired_or_invalid"
    );
    cards.bind(&mut replacement).await;
    assert_eq!(
        cards
            .send(&callback("lark", &data(&replacement, 0), "click-2"))
            .await
            .1["status"],
        "accepted"
    );
}

#[tokio::test]
async fn private_callback_recovers_kind_from_bound_card_before_current_group_policy() {
    let cards = Cards::new("lark", true).await;
    let mut prepared = cards.prepare().await;
    cards.bind(&mut prepared).await;
    cards.groups("deny").await;
    let mut click = callback("lark", &data(&prepared, 0), "private-click");
    // A claimed chat kind cannot contradict the retained originating conversation.
    click.payload["raw_platform_data"]["event"]["context"]["chat_type"] = "group".into();
    assert_eq!(cards.send(&click).await.0, 401);
    click.payload["raw_platform_data"]["event"]["context"]
        .as_object_mut()
        .unwrap()
        .remove("chat_type");
    assert_eq!(cards.send(&click).await.1["status"], "accepted");
    assert_eq!(
        cards
            .app
            .fixture
            .recorder
            .requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .activity
            .conversation
            .kind,
        ConversationKind::Private
    );
}

#[tokio::test]
async fn a_current_group_deny_keeps_the_card_unconsumed() {
    let cards = Cards::new("lark", false).await;
    let mut prepared = cards.prepare().await;
    cards.bind(&mut prepared).await;
    let click = callback("lark", &data(&prepared, 0), "group-click");
    cards.groups("deny").await;
    assert_eq!(cards.send(&click).await.1["reason"], "group_denied");
    assert!(cards
        .store
        .load(&prepared.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    cards.groups("mention").await;
    assert_eq!(cards.send(&click).await.1["status"], "accepted");
}

#[tokio::test]
async fn lark_native_card_capabilities_require_explicit_gate() {
    let mut app = App::on("lark").await;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", app.key).parse().unwrap(),
    );
    let disabled = app
        .state
        .context(AgentCaller::from_headers(&headers).unwrap(), &app.event_ref)
        .await
        .unwrap();
    assert_eq!(disabled.capabilities.buttons, "numbered_text");
    app.state.lark_cards_enabled = true;
    let enabled = app
        .state
        .context(AgentCaller::from_headers(&headers).unwrap(), &app.event_ref)
        .await
        .unwrap();
    assert_eq!(enabled.capabilities.buttons, "native_card");
    assert_eq!(enabled.capabilities.cards, "native_card");
    assert!(enabled.capabilities.edit);
}

#[tokio::test]
async fn foreign_forms_and_uncorrelated_card_actions_never_launch_or_consume() {
    let cards = Cards::new("lark", false).await;
    let mut prepared = cards.prepare().await;
    cards.bind(&mut prepared).await;
    for mutation in ["foreign", "form", "host", "value", "identity"] {
        let mut click = callback("lark", &data(&prepared, 0), "unsafe-click");
        let event = &mut click.payload["raw_platform_data"]["event"];
        match mutation {
            "foreign" => event["action"]["value"] = json!({"another_app":"value"}),
            "form" => event["action"]["form_value"] = json!({"service":"admin"}),
            "host" => event["host"] = "webpage".into(),
            "value" => event["action"]["value"]["selected"] = json!(["admin"]),
            _ => event["operator"]["open_id"] = "ou_other".into(),
        }
        sync_text(&mut click);
        let (status, body) = cards.send(&click).await;
        if matches!(mutation, "foreign" | "form") {
            assert_eq!(status, 202);
            assert_eq!(body["reason"], "interactive_action_unsupported");
        } else {
            assert_eq!(status, 401, "{mutation}: {body}");
        }
        assert!(cards
            .app
            .fixture
            .recorder
            .requests
            .lock()
            .unwrap()
            .is_empty());
        assert!(cards
            .store
            .load(&prepared.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
    }
    assert_eq!(
        cards
            .send(&callback("lark", &data(&prepared, 0), "safe-click"))
            .await
            .1["status"],
        "accepted"
    );
}
