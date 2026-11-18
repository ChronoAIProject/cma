use super::{super::replica_test_fixture::*, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    replicas::effects::EffectExecutor,
    turns::routed::{
        delivery_tests::command,
        scheduler::{Disposition, TargetExecutor, WorkItem},
        tool_reply_store::{Admission, Kind, ReplyStore},
        Phase,
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
        .admit_legacy(&h.plan, h.target(), "first", &payload)
        .await
        .unwrap();
    executor::Handler(h.tools[1].clone())
        .recover(None)
        .await
        .unwrap();
    let second = store
        .admit_legacy(&h.plan, h.target(), "second", &payload)
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
        .admit_legacy(&h.plan, h.target(), "unknown", &payload)
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
        .admit_legacy(&h.plan, h.target(), "later", &payload)
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

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_fifo_guard_unknown_saves_no_send_and_advances_head() {
    let mut h = Harness::new().await;
    let (owner, mut parent) = h.accepted().await;
    let journal = h.nodes[owner].journal();
    let guard = ReplyStore(journal.clone(), parent.partition.clone());
    let Admission::Claimed(claim) = guard
        .admit(&h.plan, h.target(), Kind::Tool, "e".repeat(64))
        .await
        .unwrap()
    else {
        panic!("guard claim");
    };
    let (mut record, version) = guard.operator_record(&h.plan, h.target()).await.unwrap();
    record["tool"] = serde_json::json!({
        "claim": uuid::Uuid::new_v4().to_string(),
        "operation": "e".repeat(64),
        "until": (journal.clock)() - 1,
        "receipt": null
    });
    journal
        .write_fenced(
            &parent.partition,
            record["id"].as_str().unwrap(),
            &h.plan.identity.owner,
            h.plan.retained_until,
            &record,
            Some(version),
        )
        .await
        .unwrap();
    drop(claim);
    journal
        .transition(
            &mut parent,
            Phase::Completed {
                response_id: "fixture-response".into(),
                durable_cursor: None,
            },
        )
        .await
        .unwrap();
    h.nodes[owner]
        .partitions
        .complete(parent.partition.as_ref().unwrap())
        .await
        .unwrap();
    // One live member without the companion forces these new occurrences into
    // the legacy FIFO path even though the other member supports lanes.
    h.nodes[1].lane_capability(false).await;
    let (capability, first_id) = super::mongo_lane_tests::pending(&h, "fifo-no-send").await;
    let later = super::mongo_progress_fixture::later_plan(&h).await;
    let fifo = h.nodes[0].head_binding(&h.plan, h.target()).await.partition;
    assert_eq!(h.nodes[0].custody(&first_id).await.unwrap().0, fifo);
    assert_eq!(
        h.nodes[0]
            .head_binding(&later, &later.targets[0])
            .await
            .partition,
        fifo
    );
    for node in &h.nodes {
        assert!(node
            .partitions
            .claim(&later, &later.targets[0])
            .await
            .is_err());
    }
    let store = Store(journal.clone());

    let (effect_owner, mut token) = super::mongo_lane_tests::claim(&h, &first_id).await;
    assert!(!token.is_lane(), "mixed cohort must pin FIFO custody");
    assert!(matches!(
        h.tools[effect_owner]
            .replica_effect_executor()
            .execute(&h.plan, h.target(), &first_id, &mut token)
            .await
            .unwrap(),
        crate::replicas::effects::EffectDisposition::Completed
    ));
    // Crash after terminal root publication but before FIFO advancement. Recovery
    // must complete an already-saved no-send Unknown too.
    h.nodes[effect_owner]
        .partitions
        .release(&token)
        .await
        .unwrap();
    execute(&h, &first_id).await;
    assert!(h.nodes[0].custody(&first_id).await.unwrap().1);
    let saved_root = store
        .read(&first_id, &h.plan, h.target())
        .await
        .unwrap()
        .unwrap();
    let saved = store.receipt(&saved_root).await.unwrap().unwrap();
    let saved_bytes = super::super::storage::bytes(&saved).unwrap();
    assert!(matches!(
        saved.delivery,
        DeliveryOutcome::Unknown { ref reason }
            if reason == "delayed_reply_guard_uncertain"
    ));
    assert_eq!(
        h.nodes[0].head_binding(&h.plan, h.target()).await.head_work,
        later.targets[0].id
    );
    use crate::turns::routed::{
        adapter_test_server::HttpServer, DispatchOutcome, HttpExecutionPort,
    };
    let http = HttpServer::start().await;
    let mut later_ran = false;
    for node in &h.nodes {
        let port = Arc::new(HttpExecutionPort::new(
            http.client(true, Duration::from_secs(5)),
            Arc::new(super::mongo_progress_fixture::Builder(h.database.keys())),
            node.plans(),
            node.journal(),
            h.current.clone(),
        ));
        match port
            .dispatcher()
            .dispatch(&later.identity, &later.targets[0].id, "later-worker")
            .await
        {
            Ok(DispatchOutcome::Replay { mut lease, .. }) => {
                later_ran = true;
                node.journal().release(&mut lease).await.unwrap();
                break;
            }
            Err(ExecutionError::Busy) => {}
            Err(error) => panic!("later dispatch failed: {error:?}"),
            Ok(_) => panic!("later dispatch did not produce a replay"),
        }
    }
    assert!(
        later_ran,
        "the queued ordinary turn on the same FIFO partition must run"
    );
    assert_eq!(http.seen().iter().filter(|r| r.method == "POST").count(), 1);
    let replay = h.tools[1]
        .reply_delayed(caller(), &capability, "fifo-no-send", message())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        super::super::storage::bytes(&replay).unwrap(),
        saved_bytes,
        "same-key replay must return the saved no-send receipt byte-for-byte"
    );
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_fifo_send_uncertainty_remains_retained() {
    let mut h = Harness::new().await;
    let (owner, mut parent) = h.accepted().await;
    h.finish_parent(owner, &mut parent).await;
    h.nodes[1].lane_capability(false).await;
    let (capability, id) = super::mongo_lane_tests::pending(&h, "fifo-send-uncertain").await;
    let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
    let ((), ()) = tokio::join!(execute(&h, &id), async {
        let mut reply = command(&mut rx).await;
        let _permit = reply.gate.attempt().await.unwrap();
        assert!(reply.gate.commit());
        // The worker is interrupted after provider IO begins; no final receipt
        // is allowed to cross the captured fence.
        let mut lost = false;
        for node in &h.nodes {
            if node.lose_work(&id).await.is_ok() {
                lost = true;
                break;
            }
        }
        assert!(lost);
        tokio::time::timeout(Duration::from_secs(30), reply.receipt.closed())
            .await
            .expect("provider receipt channel did not close after losing the lease");
        assert!(!reply.gate.commit());
        assert!(reply.receipt.send(receipt()).is_err());
    });
    let root = Store(h.nodes[0].journal())
        .read(&id, &h.plan, h.target())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(root.state, model::State::Unknown { .. }));
    assert!(h.nodes[0]
        .custody(&id)
        .await
        .is_some_and(|(_, done, _)| !done));
    let replay = h.tools[1]
        .reply_delayed(caller(), &capability, "fifo-send-uncertain", message())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
    execute(&h, &id).await;
    assert_eq!(
        h.nodes[0].head_binding(&h.plan, h.target()).await.head_work,
        id
    );
    assert!(!h.nodes[0].custody(&id).await.unwrap().1);
    assert!(
        rx.try_recv().is_err(),
        "real send uncertainty must never be resent"
    );
    h.cleanup().await;
}
