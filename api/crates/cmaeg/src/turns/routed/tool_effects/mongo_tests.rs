use super::{super::replica_test_fixture::*, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    replicas::effects::EffectExecutor,
    turns::routed::{
        delivery_tests::command,
        scheduler::{Disposition, TargetExecutor, WorkItem},
    },
};
use futures_util::future::BoxFuture;
struct NoParent;
impl TargetExecutor for NoParent {
    fn execute<'a>(&'a self, _work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { panic!("delayed effect must not reopen parent") })
    }
}
pub(super) async fn execute(h: &Harness, id: &str) {
    let work = WorkItem {
        identity: h.plan.identity.clone(),
        target_id: id.into(),
    };
    let until = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        for (node, tools) in h.nodes.iter().zip(&h.tools) {
            if node.worker.is_none() {
                continue;
            }
            let executor = node
                .journal()
                .effect_executor(
                    node.plans(),
                    Arc::new(NoParent),
                    tools.replica_effect_executor(),
                )
                .unwrap();
            if matches!(executor.execute(&work).await, Disposition::Settled) {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < until,
            "delayed custody unavailable"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_fifo_repairs_queue_gap_preserves_keys_and_replays_confirmed_receipt() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    h.finish_parent(owner, &mut lease).await;
    let capability = h.capability();
    let payload = Payload {
        capability: capability.clone(),
        caller: caller().key_hash,
        message: message(),
    };
    let store = Store(h.nodes[0].journal());
    // Crash boundary: payload/root persisted before enqueue acknowledgement.
    let first = store
        .admit(&h.plan, h.target(), "first", &payload)
        .await
        .unwrap();
    executor::Handler(h.tools[1].clone())
        .recover(None)
        .await
        .unwrap();
    let second = store
        .admit(&h.plan, h.target(), "second", &payload)
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_ne!(first.id, h.target().id);
    executor::Handler(h.tools[1].clone())
        .recover(None)
        .await
        .unwrap();
    // Repair uses stable work IDs. Repeated repair must not duplicate a queue item.
    executor::Handler(h.tools[0].clone())
        .recover(None)
        .await
        .unwrap();
    let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
    for id in [&first.id, &second.id] {
        let ((), ()) = tokio::join!(execute(&h, id), async {
            let reply = tokio::time::timeout(Duration::from_secs(5), command(&mut rx))
                .await
                .unwrap();
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt()).unwrap();
        });
        let root = store.read(id, &h.plan, h.target()).await.unwrap().unwrap();
        assert!(matches!(root.state, model::State::Settled { .. }));
    }
    // The delayed capability remains authoritative after the parent's event
    // expires; moving this reader's clock never extends the original token.
    let after_event = h.event().expires_at + 1;
    Arc::get_mut(&mut h.tools[1]).unwrap().journal.clock = Arc::new(move || after_event);
    let replay = h.tools[1]
        .reply_delayed(caller(), &capability, "first", message())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(replay.delivery, DeliveryOutcome::Confirmed { .. }));
    let mut changed = message();
    changed.text = Some("different".into());
    assert!(matches!(
        h.tools[1]
            .reply_delayed(caller(), &capability, "first", changed)
            .await,
        Err(ExecutionError::IdempotencyConflict)
    ));
    assert!(rx.try_recv().is_err());
    assert!(h.nodes[0]
        .journal()
        .effect_roots(None, 32)
        .await
        .unwrap()
        .is_empty());
    h.cleanup().await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_unknown_keeps_front_custody_and_blocks_later_sends() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    h.finish_parent(owner, &mut lease).await;
    let payload = Payload {
        capability: h.capability(),
        caller: caller().key_hash,
        message: message(),
    };
    let store = Store(h.nodes[0].journal());
    let first = store
        .admit(&h.plan, h.target(), "unknown", &payload)
        .await
        .unwrap();
    h.nodes[0]
        .journal()
        .enqueue_effect(&h.plan, h.target(), &first.id)
        .await
        .unwrap();
    let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
    let mut partial = crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
        reason: "fixture-partial-confirmation".into(),
    });
    for index in 0..80 {
        let payload: cmaeg_core::seal::MessageRefPayload =
            serde_json::from_value(serde_json::json!({
                "channel_id": h.event().channel_id, "event_id": h.event().event_id,
                "consumer": h.event().consumer, "platform_message_id": (800 + index).to_string(),
                "route_id": "telegram:77:42"
            }))
            .unwrap();
        partial
            .message_parts
            .push(crate::delivery::intents::ConfirmedMessagePart {
                index,
                message_ref: cmaeg_core::ids::MessageRef::seal(&payload, &h.database.keys())
                    .unwrap(),
            });
    }
    let ((), ()) = tokio::join!(execute(&h, &first.id), async {
        let reply = command(&mut rx).await;
        let _permit = reply.gate.attempt().await.unwrap();
        reply.receipt.send(partial.clone()).unwrap();
    });
    execute(&h, &first.id).await;
    assert!(rx.try_recv().is_err());
    let root = store
        .read(&first.id, &h.plan, h.target())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(root.state, model::State::Unknown { .. }));
    let retained = store.receipt(&root).await.unwrap().unwrap();
    assert_eq!(
        super::super::storage::bytes(&retained).unwrap(),
        super::super::storage::bytes(&partial).unwrap()
    );
    let next = store
        .admit(&h.plan, h.target(), "later", &payload)
        .await
        .unwrap();
    h.nodes[0]
        .journal()
        .enqueue_effect(&h.plan, h.target(), &next.id)
        .await
        .unwrap();
    let work = WorkItem {
        identity: h.plan.identity.clone(),
        target_id: next.id.clone(),
    };
    for (node, tools) in h.nodes.iter().zip(&h.tools) {
        let executor = node
            .journal()
            .effect_executor(
                node.plans(),
                Arc::new(NoParent),
                tools.replica_effect_executor(),
            )
            .unwrap();
        assert!(matches!(
            executor.execute(&work).await,
            Disposition::Retry(_)
        ));
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(
        h.nodes[0]
            .journal()
            .effect_roots(None, 32)
            .await
            .unwrap()
            .len(),
        2
    );
    h.cleanup().await;
}
