//! A callback has one original owner, even after restart, rename or credential rotation.
#[path = "lark_forms.rs"]
mod lark_forms;
#[path = "support.rs"]
mod support;
use self::support::Cards;
use super::{
    consumer_support::{reviewer, reviewer_key},
    relay_support::now,
};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::TurnRunner,
};
use cmaeg_core::{activity::ActivityKind, ids::IdempotencyKey, seal::open_interaction};
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn named_callbacks_restore_the_original_conversation_and_only_owner_across_restart() {
    for platform in ["telegram", "whatsapp", "lark", "feishu"] {
        let mut cards = Cards::new(platform).await;
        let card = cards.card(true).await;
        let click = cards.click(&card);
        let original = cards
            .named
            .open(&cards.app.state.keys, now())
            .unwrap()
            .payload;
        assert!(
            !cards.app.state.consumer_interactions_enabled,
            "retained reads are ungated"
        );
        cards.restart();
        let (status, first) = cards.send(&click).await;
        assert_eq!(status, 202, "{platform}: {first}");
        assert_eq!(first["status"], "accepted", "{platform}: {first}");
        cards
            .change(|v| v["routing"]["consumers"][0]["name"] = "renamed".into())
            .await;
        let mut retry = cards.click(&card);
        retry.payload["timestamp"] = "2026-10-01T00:00:00Z".into();
        assert_eq!(cards.send(&retry).await.1, first);
        let requests = cards.app.fixture.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "only one consumer per callback");
        assert_eq!(
            requests[0].activity, requests[1].activity,
            "rename/retry must not change digest"
        );
        for request in requests.iter() {
            assert_eq!(request.consumer, Some(reviewer()));
            assert_eq!(request.conversation_id, original.conversation_id);
            assert_eq!(
                request.idempotency_key,
                IdempotencyKey::for_consumer(&request.activity.event_id, &reviewer())
            );
            let routing = request.activity.routing.as_ref().unwrap();
            assert_eq!(routing.targets.len(), 1);
            assert_eq!(routing.targets[0].id, reviewer());
            let reference = request
                .event_ref
                .open(&cards.app.state.keys, now())
                .unwrap()
                .payload;
            assert_eq!(reference.consumer, Some(reviewer()));
            assert_eq!(reference.conversation_id, original.conversation_id);
            assert_eq!(reference.partition_key, original.partition_key);
            assert!(reference.verifies_activity(&request.activity));
            let ActivityKind::Interaction(result) = &request.activity.kind else {
                panic!("expected interaction")
            };
            assert_eq!(result.values["selected"], json!(["yes"]));
        }
    }
}

#[tokio::test]
async fn removed_pending_rebound_or_acl_denied_owner_never_consumes_and_can_retry() {
    for platform in ["telegram", "whatsapp", "lark", "feishu"] {
        for case in ["removed", "pending", "binding", "acl"] {
            let mut cards = Cards::new(platform).await;
            let card = cards.card(true).await;
            let original = cards.channel().await.definition.to_blob_json();
            cards.change(|v| match case {
                "removed" => {v["routing"]["consumers"] = json!([]); v["secrets"]["consumer_keys"] = json!([]);},
                "pending" => v["routing"]["consumers"][0]["pending"] = true.into(),
                "binding" => v["routing"]["consumers"][0]["provider"]["binding_id"] = "binding-replacement".into(),
                _ => v["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"allowlist","ids":["different-user"]},"chats":{"type":"open"},"groups":"deny"}),
            }).await;
            assert_ne!(
                cards.send(&cards.click(&card)).await.1["status"],
                "accepted",
                "{platform}: {case}"
            );
            assert!(cards
                .store
                .load(&card.card.id)
                .await
                .unwrap()
                .0
                .consumed
                .is_none());
            assert!(cards
                .app
                .fixture
                .recorder
                .requests
                .lock()
                .unwrap()
                .is_empty());
            cards.change(|v| *v = original).await;
            cards.restart();
            assert_eq!(
                cards.send(&cards.click(&card)).await.1["status"],
                "accepted",
                "{platform}: {case}"
            );
        }
    }
}

#[tokio::test]
async fn callback_runs_the_named_provider_with_rotated_key_and_original_conversation() {
    let cards = Cards::new("whatsapp").await;
    let card = cards.card(true).await;
    let rotated = format!("{}_rotated", reviewer_key());
    cards
        .change(|v| {
            v["secrets"]["consumer_keys"][0][1] = rotated.clone().into();
            v["routing"]["consumers"][0]["agent_key_hash"] =
                super::routing_fixture::hash(&rotated).into();
        })
        .await;
    assert_eq!(
        cards.send(&cards.click(&card)).await.1["status"],
        "accepted"
    );
    let request = cards
        .app
        .fixture
        .recorder
        .requests
        .lock()
        .unwrap()
        .pop()
        .unwrap();
    let conversation = request.conversation_id.clone();
    let (sender, mut commands) = tokio::sync::mpsc::channel(32);
    let delivery = Arc::new(cmaeg::delivery::router::ShardedScheduler::new(sender));
    let provider =
        super::fake_provider::FakeProvider::start(super::fake_provider::FakeProviderConfig {
            missing_turn_once: true,
            ..Default::default()
        })
        .await;
    let registry = cmaeg::providers::ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":1}]).to_string()).unwrap();
    let providers = Arc::new(
        cmaeg::providers::ProviderClient::new(
            registry,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            1,
        )
        .unwrap(),
    );
    let runner = TurnRunner::new(
        providers,
        cards.app.state.tracker.clone(),
        delivery,
        Default::default(),
    );
    let before = provider.log().len();
    assert!(matches!(
        runner.launch(request).await,
        LaunchOutcome::Accepted { .. }
    ));
    while let Some(command) = commands.recv().await {
        if matches!(
            command,
            cmaeg::delivery::intents::DeliveryCommand::StreamOutcome { .. }
        ) {
            break;
        }
    }
    let log = provider.log();
    let calls = &log[before..];
    assert!(calls.iter().any(|r| r.path
        == format!("/api/v1/cmaeg/bindings/binding-reviewer/conversations/{conversation}")));
    for call in calls {
        assert_eq!(
            call.authorization.as_deref(),
            Some(format!("Bearer {rotated}").as_str())
        );
    }
    let responses: Vec<_> = calls
        .iter()
        .filter(|r| r.path == "/api/v2/responses")
        .collect();
    assert_eq!(
        responses.len(),
        2,
        "missing conversation is ensured once before retry"
    );
    let body: Value = serde_json::from_str(&responses[0].body).unwrap();
    assert_eq!(body["conversation"], conversation.as_str());
}

#[tokio::test]
async fn default_callbacks_keep_legacy_record_shape_and_default_dispatch() {
    for platform in ["telegram", "whatsapp", "lark", "feishu"] {
        let cards = Cards::new(platform).await;
        let card = cards.card(false).await;
        let blob = &cards.records.records()[&card.card.id].blob;
        let bytes = open_interaction(&cards.app.state.keys, &card.card.id, blob)
            .unwrap()
            .payload;
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("callback_owner").is_none());
        assert!(value.get("consumer").is_none());
        assert_eq!(
            cards.send(&cards.click(&card)).await.1["status"],
            "accepted"
        );
        let requests = cards.app.fixture.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].consumer.is_none());
        assert!(requests[0].activity.routing.is_none());
        let original = cards
            .app
            .event_ref
            .open(&cards.app.state.keys, now())
            .unwrap()
            .payload;
        assert_eq!(requests[0].conversation_id, original.conversation_id);
    }
}

#[tokio::test]
async fn named_capabilities_require_both_platform_and_consumer_gates() {
    use cmaeg::auth::AgentCaller;
    for (platform, expected) in [
        ("telegram", "inline_keyboard"),
        ("whatsapp", "native_interactive"),
        ("lark", "native_card"),
        ("feishu", "native_card"),
    ] {
        let mut cards = Cards::new(platform).await;
        cards.app.state.lark_cards_enabled = true;
        cards.app.state.whatsapp_interactions_enabled = true;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", reviewer_key()).parse().unwrap(),
        );
        for enabled in [false, true] {
            cards.app.state.consumer_interactions_enabled = enabled;
            let context = cards
                .app
                .state
                .context(AgentCaller::from_headers(&headers).unwrap(), &cards.named)
                .await
                .unwrap();
            assert_eq!(
                context.capabilities.buttons,
                if enabled { expected } else { "numbered_text" }
            );
        }
    }
}

#[tokio::test]
async fn legacy_named_records_and_cross_source_callbacks_cannot_gain_authority() {
    use cmaeg_core::{activity::SourceKind, channel::Platform, seal::seal_consumer_interaction};
    let cards = Cards::new("telegram").await;
    let card = cards.card(true).await;
    let mut activity = cards.app.activity.clone();
    activity.source = SourceKind::NyxidRelay {
        platform: Platform::Discord,
        route_id: "route-1".into(),
    };
    let locator = card.markup["inline_keyboard"][0][0]["callback_data"]
        .as_str()
        .unwrap();
    assert!(cards
        .store
        .consume(
            cards.channel().await.as_ref(),
            &activity,
            "route-1",
            "query-1",
            locator,
            "42",
            &|_| true
        )
        .await
        .is_err());
    assert!(cards
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    let mut legacy = serde_json::to_value(&card.card).unwrap();
    legacy.as_object_mut().unwrap().remove("callback_owner");
    let blob = seal_consumer_interaction(
        &cards.app.state.keys,
        &card.card.id,
        &serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    cards.records.seed(
        &card.card.id,
        &cards.channel().await.definition.owner.subject,
        blob,
    );
    assert_eq!(
        cards.send(&cards.click(&card)).await.1["reason"],
        "interaction_expired_or_invalid"
    );
    assert!(cards
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
}
