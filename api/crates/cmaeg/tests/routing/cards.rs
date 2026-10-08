//! Card ownership is admitted before consumption and overrides every chat rule.
use super::{
    fake_cma::FakeCma,
    fake_provider::FakeProvider,
    routing_fixture::{NYX, REVIEWER},
    support::{envelope, native, App},
};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::Request,
};
use cmaeg::{
    http,
    interactions::{Destination, InteractionStore, PreparedCard},
    providers::{EventContextSnapshot, ProviderClient, ProviderRegistry, TurnRequest},
    records::RecordClient,
};
use cmaeg_core::{
    activity::{ActivityKind, RouteResolution},
    channel::ChannelDefinition,
    ids::{ConsumerId, ConversationId},
    seal::event_ref::activity_digest,
};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

struct Cards {
    app: App,
    store: Arc<InteractionStore>,
    backend: FakeCma,
    card: PreparedCard,
    relay: bool,
    _provider: FakeProvider,
}
impl Cards {
    async fn new(consumer: &str, relay: bool) -> Self {
        let app = App::new(true, |raw| {
            // A rule deliberately points elsewhere. The card must take precedence.
            raw["routing"]["rules"] = json!([{"id":"elsewhere","when":{},"targets":[NYX]}]);
        })
        .await;
        let text = format!("@{consumer} choose");
        let response = if relay {
            app.relay(&envelope(&text)).await
        } else {
            app.native(&native(&text)).await
        };
        assert_eq!(response.status(), 202);
        let request = app.recorder.requests.lock().unwrap().pop().unwrap();
        let provider = FakeProvider::start(Default::default()).await;
        let registry=ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":4}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 4)
                .unwrap(),
        );
        let consumer = request.channel.consumer(request.consumer.as_ref()).unwrap();
        let mut stream = providers
            .start_turn(
                &consumer.provider.slug,
                TurnRequest {
                    timing: None,
                    conversation_id: request.conversation_id.clone(),
                    input: vec![],
                    idempotency_key: request.idempotency_key.clone(),
                    metadata: Default::default(),
                    agent_key: consumer.credential.unwrap().clone(),
                    event_context: Some(EventContextSnapshot {
                        schema_version: 1,
                        channel_id: request.channel.id.clone(),
                        conversation_id: request.conversation_id.clone(),
                        activity: request.activity.clone(),
                        event_ref: request.event_ref.clone(),
                    }),
                },
            )
            .await
            .unwrap();
        (&mut stream.accepted).await.unwrap();
        while stream.events.recv().await.is_some() {}
        let backend = FakeCma::start(false).await;
        let store = Arc::new(InteractionStore::new(
            RecordClient::new(&backend.base_url, "cma-service").unwrap(),
            app.state.master_keys.clone(),
            providers,
        ));
        let payload = request
            .event_ref
            .open(&app.state.master_keys, super::relay_support::now())
            .unwrap()
            .payload;
        let message=serde_json::from_value(json!({"text":"Choose","blocks":[{"type":"buttons","id":"choice","options":[{"id":"yes","label":"Yes"}]}]})).unwrap();
        let mut card = store
            .prepare(&request.channel, &payload, &message)
            .await
            .unwrap();
        store
            .bind(
                &mut card,
                Destination {
                    outbound_id: uuid::Uuid::new_v4().to_string(),
                    platform_id: "42".into(),
                },
            )
            .await
            .unwrap();
        app.observer.routed.lock().unwrap().clear();
        Self {
            app,
            store,
            backend,
            card,
            relay,
            _provider: provider,
        }
    }
    async fn click(&self, enabled: bool) -> Value {
        let data = self.card.markup["inline_keyboard"][0][0]["callback_data"]
            .as_str()
            .unwrap();
        let query = json!({"id":"click-1","data":data,"from":{"id":7,"is_bot":false},"message":{"message_id":42,"date":super::relay_support::now()-1,"chat":{"id":42,"type":"private"},"text":"Choose"}});
        let mut state = self.app.state.clone().with_interactions(self.store.clone());
        state.routing.multi_consumer_enabled = enabled;
        let response = if self.relay {
            let mut value = envelope("");
            value.payload["raw_platform_data"] = json!({"callback_query":query});
            value.payload["content"] = json!({"type":"interaction"});
            value.payload["reply_to_platform_message_id"] = "42".into();
            let (body, token) = value.signed();
            let request = Request::post(format!("/v1/callbacks/{}", self.app.id))
                .header("content-type", "application/json")
                .header("x-nyxid-callback-token", token)
                .header("x-nyxid-platform", "telegram")
                .header("x-nyxid-message-id", "message-1")
                .header("x-nyxid-timestamp", super::relay_support::now_rfc3339())
                .body(Body::from(body))
                .unwrap();
            http::callbacks::callback(State(state), Path(self.app.id.clone()), request).await
        } else {
            let request = Request::post(format!("/v1/telegram/{}", self.app.id))
                .header("content-type", "application/json")
                .header("x-telegram-bot-api-secret-token", "fixture-webhook")
                .body(Body::from(
                    json!({"update_id":102,"callback_query":query}).to_string(),
                ))
                .unwrap();
            http::telegram::callback(State(state), Path(self.app.id.clone()), request).await
        };
        assert_eq!(response.status(), 202);
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap()
    }
    fn replace(&self, edit: impl FnOnce(&mut Value)) {
        let mut raw: Value =
            serde_json::from_str(&self.app.channel.definition.to_blob_json().to_string()).unwrap();
        edit(&mut raw);
        let definition: ChannelDefinition = serde_json::from_value(raw).unwrap();
        self.app.replace_definition(&definition);
    }
    async fn unconsumed(&self) {
        assert!(self
            .store
            .load(&self.card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        assert!(self.app.recorder.requests.lock().unwrap().is_empty());
        assert!(self.app.observer.routed.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_and_relay_clicks_reach_only_the_cards_consumer_with_a_bound_interaction() {
    for relay in [false, true] {
        let cards = Cards::new("reviewer", relay).await;
        assert!(cards.backend.records()[&cards.card.card.id]
            .blob
            .starts_with(b"cmaeg1.ic."));
        cards.backend.put_conflicts.store(1, Ordering::SeqCst);
        let body = cards.click(true).await;
        assert_eq!(body["status"], "accepted");
        assert_eq!(body["consumers"].as_array().unwrap().len(), 1);
        assert_eq!(body["consumers"][0]["consumer_id"], REVIEWER);
        let requests = cards.app.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        let routing = request.activity.routing.as_ref().unwrap();
        assert_eq!(routing.resolution, RouteResolution::Interaction);
        assert_eq!(routing.targets.len(), 1);
        assert_eq!(routing.targets[0].id.as_str(), REVIEWER);
        assert!(
            matches!(&request.activity.kind,ActivityKind::Interaction(result) if result.action_id=="yes")
        );
        let payload = request
            .event_ref
            .open(&cards.app.state.master_keys, super::relay_support::now())
            .unwrap()
            .payload;
        assert_eq!(payload.consumer, Some(ConsumerId::parse(REVIEWER).unwrap()));
        assert_eq!(
            payload.activity_sha256,
            Some(activity_digest(&request.activity))
        );
        assert_eq!(
            request.conversation_id,
            ConversationId::derive_for_consumer(
                &cards.app.state.master_keys,
                &cards.app.channel.id,
                1,
                &ConsumerId::parse(REVIEWER).unwrap(),
                &payload.partition_key
            )
        );
        assert_eq!(
            cards.app.observer.routed.lock().unwrap().len(),
            1,
            "CAS retries must not duplicate observation"
        );
    }
}

#[tokio::test]
async fn disabled_pending_removed_and_legacy_channels_leave_named_cards_clickable() {
    for state in ["disabled", "pending", "removed", "legacy"] {
        let cards = Cards::new("reviewer", false).await;
        cards.replace(|raw| match state {
            "pending" => raw["routing"]["consumers"][0]["pending"] = true.into(),
            "removed" => {
                raw["routing"]["consumers"]
                    .as_array_mut()
                    .unwrap()
                    .remove(0);
                raw["secrets"]["consumer_keys"]
                    .as_array_mut()
                    .unwrap()
                    .remove(0);
            }
            "legacy" => {
                raw.as_object_mut().unwrap().remove("routing");
                raw["secrets"]
                    .as_object_mut()
                    .unwrap()
                    .remove("consumer_keys");
            }
            _ => {}
        });
        let body = cards.click(state != "disabled").await;
        assert_eq!(
            body,
            json!({"status":"refused","reason":"interaction_expired_or_invalid"}),
            "{state}"
        );
        cards.unconsumed().await;
        cards.app.replace_definition(&cards.app.channel.definition);
        assert_eq!(
            cards.click(true).await["consumers"][0]["consumer_id"],
            REVIEWER
        );
        assert!(cards
            .store
            .load(&cards.card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_some());
    }
}

#[tokio::test]
async fn default_cards_override_rules_and_take_the_legacy_path_when_disabled() {
    for enabled in [false, true] {
        let cards = Cards::new("main", false).await;
        assert!(cards.backend.records()[&cards.card.card.id]
            .blob
            .starts_with(b"cmaeg1.in."));
        let body = cards.click(enabled).await;
        assert_eq!(body["status"], "accepted");
        let requests = cards.app.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].consumer.is_none());
        if enabled {
            assert_eq!(body["consumers"][0]["consumer_id"], "default");
            assert_eq!(
                requests[0].activity.routing.as_ref().unwrap().targets.len(),
                1
            );
        } else {
            assert!(body.get("consumers").is_none());
            assert!(requests[0].activity.routing.is_none());
            assert!(cards.app.observer.routed.lock().unwrap().is_empty());
        }
    }
}
