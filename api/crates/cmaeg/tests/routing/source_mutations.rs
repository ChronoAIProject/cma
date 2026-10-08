//! Source writes revoke cached ingress aliases and previously minted reply targets.
use super::{
    custom::event,
    management_support::{checked, OwnerApp},
    relay_support::Envelope,
    routing_fixture as fixture,
    support::native,
};
use cmaeg::{
    auth::{AgentCaller, Creator},
    channels::{sources, ChannelTarget},
    http::request_start::RequestStart,
    providers::{EventContextSnapshot, TurnRequest},
    tools::ToolsState,
};
use cmaeg_core::{
    channel::PartitionKey,
    ids::ChannelId,
    seal::{EventRef, ReplyTargetRef},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn creator() -> Creator {
    Creator {
        subject: "alice".into(),
        token_hash: "fixture".into(),
        organization: false,
    }
}
fn target(app: &OwnerApp) -> ChannelTarget {
    ChannelTarget::Record(app.ingress.channel.pointer.record_id.clone())
}
async fn custom_source(app: &OwnerApp) {
    sources::attach(
        &app.management,
        &creator(),
        &target(app),
        serde_json::from_value(json!({
            "expected_version":1,"source":fixture::custom(),"partition_custom_key":"event_subject"
        }))
        .unwrap(),
    )
    .await
    .unwrap();
}
async fn app() -> OwnerApp {
    let mut app = OwnerApp::new(true).await;
    app.management.routing.dynamic_sources_enabled = true;
    let mut definition = app.ingress.channel.definition.clone();
    definition.partition.key = PartitionKey::Conversation;
    app.ingress.replace_definition(&definition);
    app
}
async fn ingress(app: &OwnerApp, source: &str) -> reqwest::Response {
    match source {
        "telegram" => app.ingress.native(&native("plain message")).await,
        "relay_lark" => {
            let mut relay = Envelope::on("lark");
            relay.payload["conversation"]["id"] = fixture::ROUTE.into();
            relay.reply["conversation_id"] = fixture::ROUTE.into();
            app.ingress.relay(&relay).await
        }
        "custom" => {
            let mut event = event(None);
            event["subject"] = "order-7".into();
            app.ingress.custom(event, false).await
        }
        _ => panic!("unknown fixture source"),
    }
}
fn tools(app: &OwnerApp) -> ToolsState {
    ToolsState {
        resolver: app.management.resolver.clone(),
        keys: app.management.master_keys.clone(),
        providers: app.management.providers.clone(),
        delivery: app.ingress.state.delivery.clone(),
        tracker: app.ingress.tracker.clone(),
        public_origin: app.ingress.base.clone(),
        media: None,
        downloads: Default::default(),
        multi_consumer_enabled: true,
    }
}
fn caller() -> AgentCaller {
    AgentCaller {
        key_hash: Sha256::digest(fixture::agent_key("default").as_bytes()).into(),
    }
}

/// Store accepted ingress context in the fake provider, then mint through the real tool.
async fn delayed(tools: &ToolsState, app: &OwnerApp) -> Vec<ReplyTargetRef> {
    let turns = app
        .ingress
        .recorder
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|turn| {
            (
                turn.channel.definition.provider.slug.clone(),
                turn.event_ref.clone(),
                TurnRequest {
                    timing: None,
                    conversation_id: turn.conversation_id.clone(),
                    input: vec![],
                    idempotency_key: turn.idempotency_key.clone(),
                    metadata: Default::default(),
                    agent_key: turn
                        .channel
                        .definition
                        .secrets
                        .provider_credential
                        .clone()
                        .unwrap(),
                    event_context: Some(EventContextSnapshot {
                        schema_version: 1,
                        channel_id: turn.channel.id.clone(),
                        conversation_id: turn.conversation_id.clone(),
                        activity: turn.activity.clone(),
                        event_ref: turn.event_ref.clone(),
                    }),
                },
            )
        })
        .collect::<Vec<_>>();
    let mut targets = Vec::new();
    for (provider, reference, request) in turns {
        let mut stream = tools
            .providers
            .start_turn(&provider, request)
            .await
            .unwrap();
        (&mut stream.accepted).await.unwrap();
        while stream.events.recv().await.is_some() {}
        targets.push(
            tools
                .create_reply_target(caller(), &reference)
                .await
                .unwrap()
                .reply_target_ref,
        );
    }
    targets
}

#[tokio::test]
async fn source_detach_revokes_each_cached_alias_and_all_delayed_chat_targets() {
    for source in ["telegram", "relay_lark", "custom"] {
        let mut app = app().await;
        custom_source(&app).await;
        let aliases: Vec<ChannelId> = (0..3)
            .map(|_| {
                app.ingress
                    .channel
                    .pointer
                    .seal_id(&app.management.master_keys)
                    .unwrap()
            })
            .collect();
        for alias in &aliases {
            app.ingress.id = alias.to_string();
            checked(ingress(&app, source).await, 202).await;
        }
        app.ingress.idle().await;
        assert_eq!(app.ingress.recorder.requests.lock().unwrap().len(), 3);
        let tools = tools(&app);
        let references = if source == "custom" {
            Vec::new()
        } else {
            delayed(&tools, &app).await
        };
        // Minting forces current reads. Warm every alias again before the mutation.
        for alias in &aliases {
            app.management
                .resolver
                .resolve(alias.as_str())
                .await
                .unwrap();
        }
        sources::detach_named(&app.management, &creator(), &target(&app), source, 2)
            .await
            .unwrap();
        assert_eq!(app.management.resolver.cache_len(), 0);
        for alias in &aliases {
            app.ingress.id = alias.to_string();
            let response = ingress(&app, source).await;
            let status = response.status();
            let body: Value = response.json().await.unwrap();
            assert!(
                status.is_client_error(),
                "detached {source} accepted through an alias: {body}"
            );
            assert!(body.get("conversation_id").is_none());
        }
        for reference in references {
            let error = tools
                .reply_to_target(
                    caller(),
                    &reference,
                    "after-detach",
                    serde_json::from_value(json!({"text":"must not dispatch"})).unwrap(),
                    RequestStart(tokio::time::Instant::now()),
                )
                .await
                .unwrap_err();
            assert_eq!(error.status, 403);
            assert_eq!(error.detail.as_deref(), Some("reply_destination_revoked"));
        }
        app.ingress.idle().await;
        assert_eq!(app.ingress.recorder.requests.lock().unwrap().len(), 3);
        assert!(matches!(
            app.ingress.outcomes.as_mut().unwrap().try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        app.close().await;
    }
}

#[tokio::test]
async fn custom_attach_preserves_chat_conversations_and_detached_rules_are_inert_at_ingress() {
    let app = app().await;
    let before = checked(ingress(&app, "telegram").await, 202).await;
    custom_source(&app).await;
    let after = checked(ingress(&app, "telegram").await, 202).await;
    assert_eq!(before["conversation_id"], after["conversation_id"]);
    checked(ingress(&app, "custom").await, 202).await;
    app.ingress.idle().await;
    let reference: EventRef = app
        .ingress
        .recorder
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .event_ref
        .clone();
    let payload = reference
        .open(&app.management.master_keys, super::relay_support::now())
        .unwrap()
        .payload;
    assert!(payload.partition_key.contains("order-7"));
    let rules = json!([
        {"id":"lark-only","when":{"sources":["relay_lark"]},"targets":[fixture::NYX]},
        {"id":"mixed","when":{"sources":["relay_lark","telegram"]},"targets":[fixture::REVIEWER]}
    ]);
    let current = app.call(reqwest::Method::PUT, "/routing", Some(&json!({
        "expected_version":2,"default_consumer":{"name":"main"},"default_targets":["default"],
        "rules":rules,"max_targets":4
    })), 200).await;
    let removed = sources::detach_named(
        &app.management,
        &creator(),
        &target(&app),
        "relay_lark",
        current["version"].as_u64().unwrap(),
    )
    .await
    .unwrap();
    let stored = &removed.definition.routing.as_ref().unwrap().rules;
    assert!(!stored[0].is_active(&removed.definition));
    assert!(stored[1].is_active(&removed.definition));
    let routed = checked(ingress(&app, "telegram").await, 202).await;
    assert_eq!(routed["consumers"][0]["consumer_id"], fixture::REVIEWER);
    app.close().await;
}
