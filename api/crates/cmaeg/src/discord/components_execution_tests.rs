//! Native component through durable admission, real provider and Discord reply.
use super::{
    components_fixture::{message, multiselect_message, FixtureState},
    inbox_dispatch::InboxDispatcher,
};
use crate::{
    delivery::{
        custom::CustomDelivery, durable::DeliveryCoordinator, engine::DeliveryEngine,
        relay_client::RelayClient, router::ShardedScheduler,
    },
    http::readiness::Readiness,
    ingress::routed::RoutedIngress,
    turns::{
        routed::{
            scheduler::{ScheduleConfig, Scheduler},
            Authority, HttpExecutionPort, Journal, ProductionFrozenTurnBuilder, RoutedDelivery,
            RoutedTargetExecutor,
        },
        TurnTracker,
    },
    DeliveryConfig,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

fn frames(thread: bool) -> Vec<Value> {
    let lifecycle = |sequence, kind, status| json!({"type":kind,"sequence_number":sequence,"response":{"id":"resp_accepted","object":"response","status":status,"conversation":{"id":"agt_native"},"metadata":{},"output":[],"thread_reply":thread}});
    vec![
        lifecycle(0, "response.created", "queued"),
        json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"m","delta":"handled native component"}),
        lifecycle(2, "response.completed", "completed"),
    ]
}
#[tokio::test]
async fn native_component_runs_real_provider_once_and_replies_to_original_card() {
    run(false, false, false, false, false).await;
}
#[tokio::test]
async fn native_thread_buttons_and_selects_run_one_original_consumer_and_reply_in_same_thread() {
    for select in [false, true] {
        run(true, select, false, false, false).await;
    }
}
#[tokio::test]
async fn native_discord_multiselect_executes_one_whole_set_and_preserves_named_thread_reply() {
    for thread in [false, true] {
        run(thread, true, true, false, false).await;
    }
}
#[tokio::test]
async fn native_discord_edited_multiselect_executes_once_for_original_named_thread() {
    run(true, true, true, true, false).await;
}
#[tokio::test]
async fn discord_modal_executes_once_and_replies_to_original_named_conversation() {
    for thread in [false, true] {
        run(thread, false, false, false, true).await;
    }
}
async fn run(thread: bool, select: bool, multi: bool, edited: bool, form: bool) {
    let mut fixture = if thread {
        FixtureState::named_thread(11).await
    } else if multi || form {
        FixtureState::named().await
    } else {
        FixtureState::new().await
    };
    fixture.settle_context_message().await;
    let outbound = if multi {
        multiselect_message()
    } else {
        message(select)
    };
    let card = if form {
        super::modal_fixture::card(&fixture).await
    } else if edited {
        replaced_card(&fixture, &outbound).await
    } else {
        fixture.card(&outbound).await
    };
    let modal = if form {
        Some(super::modal_fixture::launch(&fixture, &card, 2).await.1)
    } else {
        None
    };
    let before_callback = fixture.rest.memory.calls.lock().unwrap().len();
    let mut raw = if let Some(custom) = modal {
        super::modal_fixture::submit(&fixture, &custom)
    } else {
        fixture.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        )
    };
    if thread {
        fixture.queue_thread_metadata();
    }
    if select {
        raw["data"]["component_type"] = 3.into();
        raw["data"]["values"] = if multi {
            json!(["1", "0"])
        } else {
            json!(["1"])
        };
    }
    fixture.receive(raw.clone(), 3).await.unwrap();
    {
        let mut calls = fixture.rest.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), before_callback + if thread { 2 } else { 1 });
        assert!(calls.last().unwrap().path.ends_with("/callback"));
        calls.clear();
    }
    fixture.rest.respond(
        axum::http::StatusCode::OK,
        json!({"id":"801","channel_id":"99","author":{"id":"77"}}),
    );
    let activity = fixture.activity.clone();
    let provider = &fixture.provider;
    let discord = &fixture.rest;
    let f = &mut fixture.setup;
    {
        let mut memory = provider.memory.lock().unwrap();
        memory.config.replay_frames = Some(frames(thread));
        memory.config.mirror_replay_metadata = true;
    }
    let keys = f.fixture.store.keys.clone();
    let providers = provider.client(true, Duration::from_secs(5));
    let journal = Journal::new(f.fixture.store.channels.clone(), keys.clone())
        .with_activity(activity.clone());
    let authority: Arc<dyn Authority> = Arc::new(f.state.resolver.clone());
    let port = Arc::new(HttpExecutionPort::new(
        providers.clone(),
        Arc::new(ProductionFrozenTurnBuilder::new(providers, keys.clone())),
        f.plans.clone(),
        journal.clone(),
        authority.clone(),
    ));
    let tracker = TurnTracker::new(Readiness::initialized());
    let (tx, rx) = mpsc::channel(8);
    let sink = Arc::new(ShardedScheduler::new(tx));
    let mut relay = RelayClient::new("https://nyx.example", Duration::from_secs(5)).unwrap();
    relay.discord = discord.api.clone();
    let engine = DeliveryEngine::start_observed(
        rx,
        keys.clone(),
        relay,
        DeliveryConfig::default(),
        tracker.clone(),
        CustomDelivery::new("https://gateway.example".into(), Arc::default()),
        f.state.activity.clone(),
    );
    let delivery = RoutedDelivery::new(
        f.plans.clone(),
        journal,
        authority,
        DeliveryCoordinator::new(sink),
    );
    let executor = Arc::new(RoutedTargetExecutor::new(
        port,
        delivery,
        tracker,
        &DeliveryConfig::default(),
    ));
    let (scheduler, schedule) = Scheduler::new(
        Arc::new(f.plans.clone()),
        executor,
        ScheduleConfig {
            sweep_delay: Duration::from_millis(50),
            drain_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    let source_clock = f.fixture.store.clock.clone();
    let routed = RoutedIngress::new(
        f.plans.clone(),
        schedule,
        keys.clone(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from(["cma_codex".into()]),
    )
    .with_native_discovery(f.selection.clone())
    .with_clock(Arc::new(move || source_clock() / 1000));
    f.state = f.state.clone().with_routed(Some(Arc::new(routed)));
    f.dispatcher = InboxDispatcher::with_selection(
        f.fixture.store.channels.clone(),
        keys,
        f.state.clone(),
        f.selection.clone(),
    );
    let first = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let callback_event = format!("discord:77:interaction:{}", raw["id"].as_str().unwrap());
    let receipt = first
        .items
        .iter()
        .find(|row| row.event_id == callback_event)
        .unwrap();
    assert!(receipt.consumers.is_empty());
    assert!(provider.memory.lock().unwrap().seen.is_empty());
    assert_eq!(f.sweep(f.now()).await.acknowledged, 2);
    let (stop, stopped) = watch::channel(false);
    let worker = tokio::spawn(async move { scheduler.run(stopped).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if discord.memory.calls.lock().unwrap().len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.sweep(f.now()).await.acknowledged, 2);
    stop.send(true).unwrap();
    worker.await.unwrap();
    assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
    {
        let calls = discord.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "POST");
        assert_eq!(calls[0].path, "/api/v10/channels/99/messages");
        let body: Value = serde_json::from_slice(&calls[0].body).unwrap();
        assert_eq!(body["content"], "handled native component");
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
        assert_eq!(body["message_reference"]["message_id"], "800");
    }
    {
        let memory = provider.memory.lock().unwrap();
        let turns: Vec<_> = memory
            .seen
            .iter()
            .filter(|call| call.method == "POST" && call.uri == "/api/v2/responses")
            .collect();
        assert_eq!(turns.len(), 1);
        assert!(
            memory
                .seen
                .iter()
                .all(|call| !String::from_utf8_lossy(&call.body)
                    .contains("private-interaction-token"))
        );
        if form {
            let body: Value = serde_json::from_slice(&turns[0].body).unwrap();
            let context = &body["event_context"];
            assert_eq!(
                context["conversation_id"],
                fixture.original.conversation_id.as_str()
            );
            assert_eq!(
                context["activity"]["routing"]["targets"][0]["id"],
                fixture.original.consumer.as_ref().unwrap().as_str()
            );
            assert_eq!(
                context["activity"]["kind"]["values"]["fields"],
                json!({"private-text":"Ada","private-choice":"private-write","private-number":12.5,"private-date":"2028-02-29"})
            );
        }
        if multi {
            let body: Value = serde_json::from_slice(&turns[0].body).unwrap();
            let context = &body["event_context"];
            assert_eq!(
                context["conversation_id"],
                fixture.original.conversation_id.as_str()
            );
            assert_eq!(
                context["activity"]["conversation"],
                serde_json::to_value(&fixture.context.activity.conversation).unwrap()
            );
            assert_eq!(
                context["activity"]["routing"]["targets"][0]["id"],
                fixture.original.consumer.as_ref().unwrap().as_str()
            );
            assert_eq!(context["activity"]["kind"]["block_id"], "scope");
            assert_eq!(context["activity"]["kind"]["action_id"], "scope");
            assert_eq!(
                context["activity"]["kind"]["values"]["selected"],
                json!(["private-read", "private-write"])
            );
        }
    }
    assert_eq!(f.direct_calls(), 0);
    let after = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    let completed = after
        .items
        .iter()
        .find(|row| row.event_id == callback_event)
        .unwrap();
    assert_eq!(completed.received_at, receipt.received_at);
    assert_eq!(completed.consumers.len(), 1);
}

async fn replaced_card(
    f: &FixtureState,
    message: &cmaeg_core::message::OutboundMessage,
) -> crate::interactions::PreparedCard {
    use crate::{
        delivery::{
            intents::{DeliveryOutcome, IntentReceipt},
            render::{RelayText, Renderer},
        },
        interactions::{discord_edit::Admission, Destination},
        tools::Capabilities,
    };
    let old = f.card(&super::components_fixture::message(false)).await;
    let reference = old
        .card
        .reference
        .as_ref()
        .unwrap()
        .open(&f.keys())
        .unwrap()
        .payload;
    let Admission::Ready(candidate) = f
        .store
        .discord_edit_candidate(&f.channel, &reference, message)
        .await
        .unwrap()
    else {
        panic!()
    };
    f.queue_thread_metadata();
    let credential = super::rest::token(&f.channel, "77").unwrap();
    let proof = f
        .rest
        .api
        .verify_component_channel(credential, "99", &f.context.activity.conversation)
        .await
        .unwrap();
    assert_eq!(proof, old.card.discord.as_ref().unwrap().thread);
    let mut claim = f
        .store
        .begin_discord_edit(*candidate, message)
        .await
        .unwrap();
    let (parts, rendering) = RelayText.render(
        message,
        &Capabilities {
            buttons: "inline_keyboard".into(),
            ..Capabilities::native_discord()
        },
    );
    f.rest.respond(
        axum::http::StatusCode::OK,
        json!({"id":"800","channel_id":"99","author":{"id":"77"},
        "content":parts[0].text,"components":claim.prepared.markup}),
    );
    f.rest
        .api
        .edit_components(
            credential,
            "77",
            "99",
            "800",
            &parts[0].text,
            &claim.prepared.markup,
        )
        .await
        .unwrap();
    let reference = f
        .store
        .bind(
            &mut claim.prepared,
            Destination {
                outbound_id: "800".into(),
                platform_id: "800".into(),
            },
        )
        .await
        .unwrap();
    let result = crate::interactions::PreparedCard {
        card: claim.prepared.card.clone(),
        version: claim.prepared.version,
        markup: claim.prepared.markup.clone(),
    };
    f.store
        .finish_discord_edit(
            claim,
            &IntentReceipt {
                message_parts: vec![],
                message_ref: Some(reference),
                rendering,
                delivery: DeliveryOutcome::Confirmed {
                    platform_message_id: Some("800".into()),
                },
            },
        )
        .await
        .unwrap();
    f.rest
        .respond(axum::http::StatusCode::NO_CONTENT, Value::Null);
    result
}
