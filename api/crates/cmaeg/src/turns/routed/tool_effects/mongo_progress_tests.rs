//! Replica acceptance and delayed sends retain progress across lease renewal.
//! Only the provider HTTP server and final platform transport are fixtures.
use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    turns::routed::{
        adapter_test_server::HttpServer, delivery_tests::command, DispatchOutcome, ExecutionPort,
        HttpExecutionPort,
    },
};
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn healthy_sixteen_second_delayed_send_confirms_once() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.finish_parent(owner, &mut lease).await;
            let mut capability = h.capability();
            // Membership clocks refresh asynchronously; a capability minted five
            // seconds ago is valid on both clients even across a wall-second boundary.
            capability.issued_at -= 5;
            let key = "slow-receipt";
            let admitted = h.tools[0]
                .reply_delayed(caller(), &capability, key, message())
                .await;
            assert!(
                matches!(admitted, Err(ExecutionError::Busy)),
                "admission: {admitted:?}"
            );
            let id = effect_id(h, &capability, key);
            let (sink, attempts) = transport(h, Duration::from_secs(16));
            tokio::join!(sweep(h, &id), async {
                tokio::time::sleep(Duration::from_secs(8)).await;
                let operator = crate::replicas::unresolved::Operator::new(
                    h.nodes[1].partitions.clone(),
                    h.nodes[1].plans(),
                    h.nodes[1].journal(),
                );
                assert!(
                    operator.inspect().await.unwrap().candidates.is_empty(),
                    "a live Sending effect is not abandoned or uncertain"
                );
            });
            sweep(h, &id).await;
            let result = h.tools[1]
                .reply_delayed(caller(), &capability, key, message())
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
                "#2171 PR A: healthy 16-second send must retain confirmation: {:?}",
                result.delivery
            );
            assert_eq!(
                attempts.load(Ordering::SeqCst),
                1,
                "receipt loss cannot authorize a resend"
            );
            sink.abort();
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn provider_acceptance_after_fifteen_seconds_remains_committable_without_repost() {
    with_harness(|h| Box::pin(async move {
    let (owner, mut lease) = h.acquire().await;
    let journal = h.nodes[owner].journal();
    journal.release(&mut lease).await.unwrap();
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.post_delay = Duration::from_secs(16);
    let port = Arc::new(HttpExecutionPort::new(
        http.client(true, Duration::from_secs(30)),
        Arc::new(Builder(h.database.keys())),
        h.nodes[owner].plans(),
        journal.clone(),
        h.current.clone(),
    ));
    assert!(
        !port
            .support(&h.target().launch.provider)
            .exact_body_idempotency
    );
    let first = port
        .clone()
        .dispatcher()
        .dispatch(&h.plan.identity, &h.target().id, "slow-acceptance")
        .await;
    let accepted = matches!(&first, Ok(DispatchOutcome::Replay { response_id, .. }) if response_id == "resp_accepted");
    let first_error = first.as_ref().err().cloned();
    if let Ok(DispatchOutcome::Replay { mut lease, .. }) = first {
        journal.release(&mut lease).await.unwrap();
    }
    // Placement can change after the second node's membership becomes visible.
    // Recovery must visit both independent clients, like the production scan.
    let (recovery_owner, recovered) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for (index, node) in h.nodes.iter().enumerate() {
                let recovery = Arc::new(HttpExecutionPort::new(
                    http.client(true, Duration::from_secs(30)),
                    Arc::new(Builder(h.database.keys())),
                    node.plans(),
                    node.journal(),
                    h.current.clone(),
                ));
                let result = recovery
                    .dispatcher()
                    .dispatch(&h.plan.identity, &h.target().id, "recovery")
                    .await;
                if !matches!(result, Err(ExecutionError::Busy)) {
                    return (index, result);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("a live replica must recover the expired claim");
    let recoverable = matches!(&recovered, Ok(DispatchOutcome::Replay { response_id, .. }) if response_id == "resp_accepted");
    if let Ok(DispatchOutcome::Replay { mut lease, .. }) = recovered {
        h.nodes[recovery_owner]
            .journal()
            .release(&mut lease)
            .await
            .unwrap();
    }
    let (state, _) = journal
        .read::<TargetState>(&h.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(http.seen().iter().filter(|r| r.method == "POST").count(), 1);
    assert!(accepted && recoverable, "#2171: a valid acceptance inside the 20-second budget was discarded: first={first_error:?}, retained phase={:?}", state.phase);
    })).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_reply_behind_unknown_head_eventually_returns_receipt_without_parent_settlement() {
    with_harness(|h| Box::pin(async move {
    let (owner, mut lease) = h.accepted().await;
    let journal = h.nodes[owner].journal();
    journal
        .transition(&mut lease, Phase::Unknown)
        .await
        .unwrap();
    journal.release(&mut lease).await.unwrap();
    let mut capability = h.capability();
    // Membership clocks refresh asynchronously; a capability minted five
    // seconds ago is valid on both clients even across a wall-second boundary.
    capability.issued_at -= 5;
    let key = "behind-unknown";
    let admitted = h.tools[0]
        .reply_delayed(caller(), &capability, key, message())
        .await;
    assert!(
        matches!(admitted, Err(ExecutionError::Busy)),
        "admission: {admitted:?}"
    );
    // Busy is the existing REST 409 routed_reply_in_progress mapping. Retrying
    // the SAME admitted occurrence must eventually produce a terminal receipt.
    let id = effect_id(h, &capability, key);
    let (sink, attempts) = transport(h, Duration::ZERO);
    let mut result = Err(ExecutionError::Busy);
    let mut pending = 0;
    for _ in 0..3 {
        sweep(h, &id).await;
        result = h.tools[1]
            .reply_delayed(caller(), &capability, key, message())
            .await;
        if !matches!(result, Err(ExecutionError::Busy)) {
            break;
        }
        pending += 1;
    }
    let (parent, _) = journal
        .read::<TargetState>(&h.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parent.phase,
        Phase::Unknown,
        "reply must not settle the unknown parent"
    );
    assert!(
        attempts.load(Ordering::SeqCst) <= 1,
        "same key must never send twice"
    );
    let sent = attempts.load(Ordering::SeqCst);
    sink.abort();
    let confirmed = matches!(&result, Ok(Some(receipt)) if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }));
    assert_eq!((confirmed, sent), (true, 1), "#2171 PR B: {pending} same-key retries: {result:?}");
    })).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn forced_delayed_send_loss_retains_unknown_without_resend_or_blocking_later_turn() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.finish_parent(owner, &mut lease).await;
            let mut capability = h.capability();
            capability.issued_at -= 5;
            let key = "forced-loss";
            let admitted = h.tools[0]
                .reply_delayed(caller(), &capability, key, message())
                .await;
            assert!(
                matches!(admitted, Err(ExecutionError::Busy)),
                "admission: {admitted:?}"
            );
            let id = effect_id(h, &capability, key);
            let later = later_plan(h).await;
            let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
            let reply = {
                let execution = sweep(h, &id);
                tokio::pin!(execution);
                let reply = tokio::select! {
                    _ = &mut execution => panic!("send must await its receipt"),
                    reply = command(&mut rx) => reply,
                };
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit()); // Exactly one simulated platform request.
                reply
                // Abort the owning future after IO begins. Even a future renewal
                // driver must die here; this is not a healthy but slow send.
            };
            assert!(!reply.gate.commit());
            tokio::time::sleep(Duration::from_secs(16)).await;
            sweep(h, &id).await;
            assert!(reply.gate.attempt().await.is_err());
            assert!(
                reply.receipt.send(receipt()).is_err(),
                "late worker cannot commit"
            );
            for _ in 0..2 {
                let saved = h.tools[1]
                    .reply_delayed(caller(), &capability, key, message())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(matches!(saved.delivery, DeliveryOutcome::Unknown { .. }));
                sweep(h, &id).await;
            }
            assert!(
                rx.try_recv().is_err(),
                "forced loss must never cause a second send"
            );
            let mut progressed = false;
            for node in &h.nodes {
                if let Ok(mut next) = node
                    .journal()
                    .claim(&later, &later.targets[0], "later-worker")
                    .await
                {
                    progressed = true;
                    node.journal().release(&mut next).await.unwrap();
                    break;
                }
            }
            assert!(
                progressed,
                "#2171 PR B: retained-unknown effect still blocks the later turn"
            );
        })
    })
    .await;
}
