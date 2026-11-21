//! Confirmed outbound reaction through provider execution and the original Discord thread.
use super::{inbox_dispatch::InboxDispatcher, rest_fixture::Fixture};
use crate::{
    delivery::{
        custom::CustomDelivery, durable::DeliveryCoordinator, engine::DeliveryEngine,
        relay_client::RelayClient, router::ShardedScheduler,
    },
    http::readiness::Readiness,
    ingress::routed::RoutedIngress,
    turns::{
        routed::{
            adapter_test_server::HttpServer,
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

fn frames() -> Vec<Value> {
    let lifecycle = |sequence, kind, status| json!({"type":kind,"sequence_number":sequence,"response":{"id":"resp_accepted","object":"response","status":status,"conversation":{"id":"agt_native"},"metadata":{},"output":[],"thread_reply":true}});
    vec![
        lifecycle(0, "response.created", "queued"),
        json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"m","delta":"hello native Discord"}),
        lifecycle(2, "response.completed", "completed"),
    ]
}
#[tokio::test]
async fn discord_outbound_reaction_executes_once_in_original_provider_conversation_and_thread() {
    let mut f = super::reaction_tests::setup().await;
    f.channel.partition.key = cmaeg_core::channel::PartitionKey::Thread;
    f.fixture.replace(&f.channel);
    let native = Arc::get_mut(&mut f.journal).unwrap();
    native.api.thread_metadata = true;
    native.authority.public_definition["partition"]["key"] = "thread".into();
    f.fixture.memory.discord_channels.lock().unwrap().insert(
        "100".into(),
        json!({"id":"100","type":11,"guild_id":"88","parent_id":"99"}),
    );
    let activity = f.observe();
    let provider = HttpServer::start().await;
    {
        let mut memory = provider.memory.lock().unwrap();
        memory.config.replay_frames = Some(frames());
        memory.config.mirror_replay_metadata = true;
    }
    let discord = Fixture::new().await;
    for id in ["800", "801", "802"] {
        discord.memory.responses.lock().unwrap().push_back((
            axum::http::StatusCode::OK,
            json!({"id":id,"channel_id":"100","author":{"id":"77"}}),
        ));
    }
    let keys = f.fixture.store.keys.clone();
    let providers = provider.client(true, Duration::from_secs(5));
    let journal = Journal::new(f.fixture.store.channels.clone(), keys.clone())
        .with_activity(activity.clone());
    let authority: Arc<dyn Authority> = Arc::new(f.state.resolver.clone());
    let port = Arc::new(HttpExecutionPort::new(
        providers.clone(),
        Arc::new(ProductionFrozenTurnBuilder::new(
            providers.clone(),
            keys.clone(),
        )),
        f.plans.clone(),
        journal.clone(),
        authority.clone(),
    ));
    let tracker = TurnTracker::new(Readiness::initialized());
    let (tx, rx) = mpsc::channel(8);
    let sink = Arc::new(ShardedScheduler::new(tx));
    let mut relay = RelayClient::new("https://nyx.example", Duration::from_secs(5)).unwrap();
    relay.discord = discord.api.clone();
    relay = relay
        .with_discord_reactions(true)
        .with_interactions(Arc::new(crate::interactions::InteractionStore::new(
            f.fixture.store.channels.clone(),
            keys.clone(),
            providers,
        )));
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
            // The pinned fake provider exposes one response ID/metadata slot.
            concurrency: 1,
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
        BTreeSet::from([f.fixture.record.clone()]),
        BTreeSet::new(),
        BTreeSet::from(["cma_codex".into()]),
    )
    .with_clock(Arc::new(move || source_clock() / 1000));
    f.state = f.state.clone().with_routed(Some(Arc::new(routed)));
    f.dispatcher = InboxDispatcher::new(
        f.fixture.store.channels.clone(),
        keys,
        f.state.clone(),
        BTreeSet::from([f.fixture.record.clone()]),
    );
    f.message(|event| {
        event["content"] = "hello".into();
        event["channel_id"] = "100".into();
        event["guild_id"] = "88".into();
    })
    .await;
    let first = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.items[0].consumers.is_empty());
    assert!(provider.memory.lock().unwrap().seen.is_empty());
    let inbox = super::inbox_scan::InboxStore::new(
        f.fixture.store.channels.clone(),
        f.fixture.store.keys.clone(),
    );
    let entry = inbox.page(None, f.now()).await.unwrap().entries.remove(0);
    f.fixture.fail(&entry.ack_id(), false);
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    let mut change = super::reaction_tests::event(&f, "MESSAGE_REACTION_ADD", 2);
    change.data["message_id"] = "800".into();
    change.data["channel_id"] = "100".into();
    change.data["message_author_id"] = "77".into();
    super::reaction_tests::persist(&f, &change, "gateway-a")
        .await
        .unwrap();
    super::reaction_tests::persist(&f, &change, "gateway-a")
        .await
        .unwrap();
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert_eq!(
        f.plans
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
    assert!(provider.memory.lock().unwrap().seen.is_empty());
    let (stop, stopped) = watch::channel(false);
    let worker = tokio::spawn(async move { scheduler.run(stopped).await });
    let receipt_id = super::receipts::id(&f.fixture.record, "77", "100", "800");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if f.fixture
                .store
                .load::<super::receipts::Receipt>(&receipt_id, true)
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("original reply must have exact confirmed native custody");
    f.fixture.advance(1000);
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if discord.memory.calls.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if result.is_err() {
        let state = activity
            .latest(&f.channel.owner.subject, &f.fixture.record)
            .await
            .unwrap();
        let calls: Vec<_> = provider
            .memory
            .lock()
            .unwrap()
            .seen
            .iter()
            .map(|call| (call.method.clone(), call.uri.clone()))
            .collect();
        panic!(
            "provider calls {calls:?}; deliveries {}; activity {state:?}",
            discord.memory.calls.lock().unwrap().len()
        );
    }
    assert_eq!(f.sweep(f.now()).await.acknowledged, 2);
    // A bot reply to a reaction references a retained Event, not a Message.
    // Its next reaction must still inherit the exact original consumer/thread.
    let chained_receipt = super::receipts::id(&f.fixture.record, "77", "100", "801");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if f.fixture
                .store
                .load::<super::receipts::Receipt>(&chained_receipt, true)
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reaction reply must retain confirmed custody");
    f.fixture
        .advance((time::OffsetDateTime::now_utc().unix_timestamp() - f.now()).max(0) * 1000);
    let mut chained = super::reaction_tests::event(&f, "MESSAGE_REACTION_ADD", 3);
    chained.data["message_id"] = "801".into();
    chained.data["channel_id"] = "100".into();
    chained.data["message_author_id"] = "77".into();
    super::reaction_tests::persist(&f, &chained, "gateway-a")
        .await
        .unwrap();
    super::reaction_tests::persist(&f, &chained, "gateway-a")
        .await
        .unwrap();
    assert_eq!(f.sweep(f.now()).await.acknowledged, 3);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if discord.memory.calls.lock().unwrap().len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("chained reaction must execute and reply exactly once");
    stop.send(true).unwrap();
    worker.await.unwrap();
    assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
    {
        let calls = discord.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls
            .iter()
            .all(|call| call.path == "/api/v10/channels/100/messages"));
        assert_eq!(calls[0].method, "POST");
        assert_eq!(calls[0].path, "/api/v10/channels/100/messages");
        let body: Value = serde_json::from_slice(&calls[0].body).unwrap();
        assert_eq!(body["content"], "hello native Discord");
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
        assert!(body["message_reference"]["message_id"].is_string());
    }
    assert_eq!(
        provider
            .memory
            .lock()
            .unwrap()
            .seen
            .iter()
            .filter(|call| call.method == "POST" && call.uri == "/api/v2/responses")
            .count(),
        3
    );
    {
        let memory = provider.memory.lock().unwrap();
        let requests: Vec<_> = memory
            .seen
            .iter()
            .filter(|call| call.method == "POST" && call.uri == "/api/v2/responses")
            .collect();
        let bodies: Vec<Value> = requests
            .iter()
            .map(|call| serde_json::from_slice(&call.body).unwrap())
            .collect();
        assert_eq!(bodies.len(), 3);
        assert!(bodies
            .iter()
            .all(|body| body["conversation"] == bodies[0]["conversation"]));
        let encoded = serde_json::to_string(&bodies).unwrap();
        assert!(encoded.contains("cma.message.reaction.updated.v1"));
    }
    assert_eq!(f.direct_calls(), 0);
    assert_eq!(&*f.fixture.memory.channel_calls.lock().unwrap(), &["100"]);
    let mut cursor = None;
    let mut retained = Vec::new();
    loop {
        let page = f
            .plans
            .recover_all_page(cursor.as_deref(), 32)
            .await
            .unwrap();
        retained.extend(page.plans);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(retained.len() <= 3);
    }
    assert_eq!(retained.len(), 3);
    let after = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    let original_activity = after
        .items
        .iter()
        .find(|item| item.event_id == first.items[0].event_id)
        .unwrap();
    assert_eq!(original_activity.received_at, first.items[0].received_at);
    assert!(!after.items[0].consumers.is_empty());
}
