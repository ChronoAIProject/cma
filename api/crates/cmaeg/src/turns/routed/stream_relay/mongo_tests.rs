//! Real shared records, distinct clients/registries, and the production relay router.
use super::*;
use crate::{
    delivery::{
        custom::CustomDelivery, durable::DeliveryCoordinator, router::ShardedScheduler,
        sync::SyncRegistry,
    },
    ingress::{routing_plan::fixture, routing_store::Candidate},
    replicas::mongo_fixture::Fixture,
    turns::routed::{
        test_support::{self, Current},
        FrameDelivery, FrozenTurn, Phase, ReplayEffect, RoutedToolReplies,
    },
};
use axum::{
    body::to_bytes,
    response::{IntoResponse, Sse},
};
use cmaeg_core::seal::EventRef;
use serde_json::json;
use std::sync::{atomic::AtomicUsize, Arc, Mutex};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn remote_executor_delivers_once_to_original_stream_over_authenticated_http() {
    run(false).await;
    run(true).await;
}

async fn run(drop_ack: bool) {
    let database = Fixture::new().await;
    let nodes = [database.node(31).await, database.node(32).await];
    let instances: Vec<_> = nodes
        .iter()
        .map(|node| node.partitions.membership().instance)
        .collect();
    tokio::time::timeout(Duration::from_secs(15), async {
        while nodes.iter().any(|node| {
            instances
                .iter()
                .any(|id| node.partitions.membership().origin(id).is_err())
        }) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    // The single active partition goes to the first member's quota. Keep the
    // original HTTP stream on the other member, deterministically.
    let executor = usize::from(instances[0] > instances[1]);
    let origin = 1 - executor;
    let registries: Vec<_> = instances
        .iter()
        .map(|id| Arc::new(SyncRegistry::default().with_instance(id.clone())))
        .collect();
    let (waiter_id, waiter) = registries[origin].register().unwrap();
    let mut request = fixture::request(
        |value| {
            value["routing"] = serde_json::Value::Null;
            value["secrets"]["consumer_keys"] = json!([]);
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
        },
        fixture::custom(None),
    );
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let keys = database.keys();
    let mut event = request
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload;
    event.expires_at = now + 900;
    event.conversation_id = cmaeg_core::ids::ConversationId::derive(
        &keys,
        &request.channel.id,
        1,
        &event.partition_key,
    );
    event.reply = ReplyAddress::Sync { waiter: waiter_id };
    request.conversation_id = event.conversation_id.clone();
    request.event_ref = EventRef::seal(&event, &keys).unwrap();
    let candidate =
        Candidate::prepare_compatibility(&request, b"stream-fixture", &keys, now).unwrap();
    let plan = nodes[origin].plans().admit(&candidate).await.unwrap();
    let target = &plan.targets[0];
    let current = Arc::new(Current {
        channel: Mutex::new(request.channel),
        calls: AtomicUsize::new(0),
        revoke_at: AtomicUsize::new(usize::MAX),
    });
    let journal = nodes[executor].journal();
    let mut lease = journal.claim(&plan, target, "stream-test").await.unwrap();
    assert_eq!(
        lease.partition.as_ref().unwrap().fence().instance,
        instances[executor]
    );
    let mut body = test_support::frozen(&plan, target, &current.channel.lock().unwrap(), 32);
    body.event_context.event_ref = FrozenTurn::event_ref(&plan, target, &keys).unwrap();
    journal
        .freeze(&mut lease, &plan, target, &body)
        .await
        .unwrap();
    // Provider acceptance is a simulated setup boundary. Everything from the
    // actual pending delivery intent through HTTP and acknowledgement is real.
    journal
        .transition(
            &mut lease,
            Phase::Accepted {
                response_id: "stream-response".into(),
                durable_cursor: None,
            },
        )
        .await
        .unwrap();
    let (tx, mut effects) = tokio::sync::mpsc::channel(8);
    let coordinator = |index: usize| {
        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx.clone()))).with_custom_stream(
            CustomDelivery::new(
                "https://replica-fixture.invalid".into(),
                registries[index].clone(),
            ),
        )
    };
    let receiver = Arc::new(
        RoutedToolReplies::new(
            nodes[origin].plans(),
            nodes[origin].journal(),
            current.clone(),
            coordinator(origin),
        )
        .with_replica_relay()
        .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", 20031 + origin))
        .await
        .unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let router = receiver
        .clone()
        .replica_relay_router()
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| async move {
                let response = next.run(request).await;
                if drop_ack {
                    tokio::time::sleep(WINDOW + Duration::from_secs(1)).await;
                }
                response
            },
        ));
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let delivery = RoutedDelivery::new(
        nodes[executor].plans(),
        journal.clone(),
        current,
        coordinator(executor),
    )
    .with_stream_relay()
    .unwrap();
    assert!(delivery.remote_waiter(&event));
    let frame = test_support::recovery_frame("stream-response", 0, true);
    let effect = ReplayEffect::Custom(CustomStreamEffect::Completed {
        final_text: Some("remote answer".into()),
    });
    let outcome = delivery
        .deliver_frame(
            &plan.identity,
            &target.id,
            &mut lease,
            &frame,
            effect.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, FrameDelivery::Settled { cursor: 0, .. }));
    assert_eq!(
        lease
            .state
            .delivery
            .as_ref()
            .unwrap()
            .custom
            .as_ref()
            .unwrap()
            .local,
        Some(if drop_ack {
            LocalStreamOutcome::Lost
        } else {
            LocalStreamOutcome::Buffered
        })
    );
    let text = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(Sse::new(waiter.stream()).into_response().into_body(), 10000),
    )
    .await
    .unwrap()
    .unwrap();
    let text = std::str::from_utf8(&text).unwrap();
    assert_eq!(text.matches("event: cmaeg.completed").count(), 1);
    assert!(text.contains("remote answer"));
    delivery
        .deliver_frame(&plan.identity, &target.id, &mut lease, &frame, effect)
        .await
        .unwrap();
    assert!(effects.try_recv().is_err());
    assert!(registries[origin].is_empty());
    stop.send(()).unwrap();
    server.await.unwrap();
    drop(receiver);
    drop(delivery);
    drop(journal);
    drop(nodes);
    database.cleanup().await;
}
