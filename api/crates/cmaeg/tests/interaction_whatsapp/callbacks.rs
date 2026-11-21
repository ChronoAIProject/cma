//! Signed NyxID callbacks traverse real verification, ACL and encrypted CAS.
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
use cmaeg_core::{activity::ActivityKind, message::OutboundMessage};
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

struct Cards {
    app: App,
    records: FakeCma,
    store: Arc<InteractionStore>,
}
impl Cards {
    async fn new() -> Self {
        let app = App::on_private("whatsapp").await;
        let records = FakeCma::start(false).await;
        let store = Arc::new(InteractionStore::new(
            RecordClient::new(&records.base_url, "cma-service").unwrap(),
            app.state.keys.clone(),
            app.state.providers.clone(),
        ));
        Self {
            app,
            records,
            store,
        }
    }
    async fn prepare(&self, list: bool) -> PreparedCard {
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
        let mut block = json!({"type":"buttons","id":"services","options":[{"id":"a","label":"A"},{"id":"b","label":"B"}]});
        if list {
            block["type"] = "choice".into();
            block["prompt"] = "Service".into();
            block["multi"] = false.into();
        }
        let message: OutboundMessage =
            serde_json::from_value(json!({"text":"Choose", "blocks":[block]})).unwrap();
        self.store
            .prepare(&channel, &event, &message)
            .await
            .unwrap()
    }
    async fn bind(&self, prepared: &mut PreparedCard) {
        self.store
            .bind(
                prepared,
                Destination {
                    outbound_id: "outbound-1".into(),
                    platform_id: "wamid.card".into(),
                },
            )
            .await
            .unwrap();
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
    async fn acl(&self, deny: bool) {
        let channel = self
            .app
            .state
            .resolver
            .resolve(self.app.fixture.id.as_str())
            .await
            .unwrap();
        let mut definition = channel.definition.clone();
        let mut source = serde_json::to_value(&definition.sources[0]).unwrap();
        source["admission"] = json!({"type":"scoped","senders":if deny {json!({"type":"allowlist","ids":["8"]})} else {json!({"type":"open"})},"chats":{"type":"open"},"groups":"deny"});
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
        self.app
            .fixture
            .state
            .resolver
            .invalidate(self.app.fixture.id.as_str());
    }
}
fn data(card: &PreparedCard, index: usize) -> String {
    let action = &card.markup["action"];
    if card.markup["type"] == "button" {
        action["buttons"][index]["reply"]["id"]
            .as_str()
            .unwrap()
            .into()
    } else {
        action["sections"][0]["rows"][index]["id"]
            .as_str()
            .unwrap()
            .into()
    }
}
fn callback(data: &str, id: &str) -> Envelope {
    let mut e = Envelope::on("whatsapp");
    let kind = if data.starts_with("cw1b:") {
        "button_reply"
    } else {
        "list_reply"
    };
    e.payload["conversation"]["platform_id"] = "7".into();
    e.payload["reply_to_platform_message_id"] = "wamid.card".into();
    e.payload["content"] = json!({"type":"text","text":"Forged title is not action authority"});
    e.payload["raw_platform_data"] = json!({"id":id,"from":"7","type":"interactive","context":{"id":"wamid.card"},"interactive":{"type":kind,kind:{"id":data,"title":"Forged title is not action authority"}}});
    e
}

#[tokio::test]
async fn button_and_list_callbacks_survive_restart_keep_exact_values_and_deduplicate() {
    for list in [false, true] {
        let mut cards = Cards::new().await;
        let mut prepared = cards.prepare(list).await;
        let click = callback(&data(&prepared, 1), "wamid.click");
        assert_eq!(
            cards.send(&click).await.1["reason"],
            "interaction_expired_or_invalid"
        );
        cards.bind(&mut prepared).await;
        cards.store = Arc::new(InteractionStore::new(
            RecordClient::new(&cards.records.base_url, "cma-service").unwrap(),
            cards.app.state.keys.clone(),
            cards.app.state.providers.clone(),
        ));
        let (first, second) = tokio::join!(cards.send(&click), cards.send(&click));
        assert_eq!(first.0, 202);
        assert_eq!(first.1["status"], "accepted");
        assert_eq!(first, second);
        let mut later_retry = callback(&data(&prepared, 1), "wamid.click");
        later_retry.payload["timestamp"] = "2026-10-01T00:00:00Z".into();
        assert_eq!(cards.send(&later_retry).await.1, first.1);
        {
            let requests = cards.app.fixture.recorder.requests.lock().unwrap();
            let ActivityKind::Interaction(result) = &requests.last().unwrap().activity.kind else {
                panic!("not interaction");
            };
            assert_eq!(result.block_id, "services");
            assert_eq!(result.action_id, "b");
            assert_eq!(result.values["selected"], json!(["b"]));
            assert_eq!(result.values["revision"], 1);
            assert_eq!(result.values["platform_message_id"], "wamid.card");
            assert_eq!(requests[0].activity, requests.last().unwrap().activity);
        }
        assert_eq!(
            cards
                .send(&callback(&data(&prepared, 1), "wamid.other-click"))
                .await
                .1["reason"],
            "interaction_expired_or_invalid"
        );
        assert_eq!(
            cards
                .send(&callback(&data(&prepared, 0), "wamid.click"))
                .await
                .1["reason"],
            "interaction_expired_or_invalid"
        );
    }
}

#[tokio::test]
async fn forged_actor_destination_revision_action_and_control_kind_leave_card_unused() {
    let cards = Cards::new().await;
    let mut prepared = cards.prepare(false).await;
    cards.bind(&mut prepared).await;
    let locator = data(&prepared, 0);
    for case in 0..9 {
        let mut click = callback(&locator, "wamid.click");
        match case {
            0 => {
                click.payload["raw_platform_data"]["from"] = "8".into();
                click.payload["sender"]["platform_id"] = "8".into();
                click.payload["conversation"]["platform_id"] = "8".into();
            }
            1 => {
                click.payload["raw_platform_data"]["context"]["id"] = "wamid.other".into();
                click.payload["reply_to_platform_message_id"] = "wamid.other".into();
            }
            2 => click = callback(&locator.replacen(":1:0", ":2:0", 1), "wamid.click"),
            3 => click = callback(&locator.replacen(":1:0", ":1:7", 1), "wamid.click"),
            4 => click = callback(&locator.replacen("cw1b:", "cw1l:", 1), "wamid.click"),
            5 => click.payload["sender"]["platform_id"] = "8".into(),
            6 => click.payload["reply_to_platform_message_id"] = "wamid.other".into(),
            7 => click.payload["raw_platform_data"]["interactive"]["type"] = "list_reply".into(),
            _ => {
                click.payload["raw_platform_data"]["interactive"]["list_reply"] =
                    json!({"id":locator,"title":"extra"})
            }
        }
        let (_, body) = cards.send(&click).await;
        assert_ne!(body["status"], "accepted", "case {case}");
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
        cards.send(&callback(&locator, "wamid.click")).await.1["status"],
        "accepted"
    );
}

#[tokio::test]
async fn changed_acl_and_transient_store_failure_allow_a_later_valid_retry() {
    let cards = Cards::new().await;
    let mut prepared = cards.prepare(false).await;
    cards.bind(&mut prepared).await;
    let click = callback(&data(&prepared, 0), "wamid.click");
    cards.acl(true).await;
    assert_eq!(cards.send(&click).await.1["status"], "refused");
    assert!(cards
        .store
        .load(&prepared.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    cards.acl(false).await;
    cards.records.fail_put.store(true, Ordering::SeqCst);
    assert_eq!(cards.send(&click).await.0, 503);
    cards.records.fail_put.store(false, Ordering::SeqCst);
    cards.records.put_conflicts.store(2, Ordering::SeqCst);
    assert_eq!(cards.send(&click).await.0, 503);
    assert!(cards
        .store
        .load(&prepared.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    cards.records.put_conflicts.store(1, Ordering::SeqCst);
    assert_eq!(cards.send(&click).await.1["status"], "accepted");
}

#[tokio::test]
async fn foreign_controls_stay_content_and_unknown_issued_locators_never_gain_authority() {
    let cards = Cards::new().await;
    let foreign = callback("external-option", "wamid.foreign");
    assert_eq!(cards.send(&foreign).await.1["status"], "accepted");
    assert!(matches!(
        cards.app.fixture.recorder.requests.lock().unwrap()[0]
            .activity
            .kind,
        ActivityKind::Message { .. }
    ));
    let unknown = callback("cw1l:00000000000040008000000000000000:1:0", "wamid.unknown");
    assert_eq!(
        cards.send(&unknown).await.1["reason"],
        "interaction_expired_or_invalid"
    );
}

#[tokio::test]
async fn capability_requires_verified_raw_private_context_and_explicit_issuance_gate() {
    let mut cards = Cards::new().await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", cards.app.key).parse().unwrap(),
    );
    for enabled in [false, true] {
        cards.app.state.whatsapp_interactions_enabled = enabled;
        let context = cards
            .app
            .state
            .context(
                AgentCaller::from_headers(&headers).unwrap(),
                &cards.app.event_ref,
            )
            .await
            .unwrap();
        assert_eq!(
            context.capabilities.buttons,
            if enabled {
                "native_interactive"
            } else {
                "numbered_text"
            }
        );
        assert!(!context.capabilities.edit);
        assert!(context.activity_verified);
    }
}

#[tokio::test]
async fn wrong_source_route_expiry_and_unsupported_edit_do_not_authorize_callbacks() {
    let cards = Cards::new().await;
    let mut prepared = cards.prepare(false).await;
    cards.bind(&mut prepared).await;
    let channel = cards
        .app
        .state
        .resolver
        .resolve(cards.app.fixture.id.as_str())
        .await
        .unwrap();
    let mut activity = cards.app.activity.clone();
    activity.source = cmaeg_core::activity::SourceKind::NyxidRelay {
        platform: cmaeg_core::channel::Platform::Whatsapp,
        route_id: "different-route".into(),
    };
    assert!(cards
        .store
        .consume(
            &channel,
            &activity,
            "route-1",
            "wamid.click",
            &data(&prepared, 0),
            "wamid.card",
            &|c| c.is_none()
        )
        .await
        .is_err());
    let reference = prepared
        .card
        .reference
        .as_ref()
        .unwrap()
        .open(&cards.app.state.keys)
        .unwrap()
        .payload;
    assert!(cards
        .store
        .replacement(&channel, &reference, &prepared.card.message)
        .await
        .is_err());
    let mut expired = prepared.card.clone();
    expired.expires_at = now() - 1;
    let blob = cmaeg_core::seal::seal_interaction(
        &cards.app.state.keys,
        &expired.id,
        &serde_json::to_vec(&expired).unwrap(),
    )
    .unwrap();
    cards
        .records
        .seed(&expired.id, &channel.definition.owner.subject, blob);
    assert_eq!(
        cards
            .send(&callback(&data(&prepared, 0), "wamid.click"))
            .await
            .1["reason"],
        "interaction_expired_or_invalid"
    );
}
