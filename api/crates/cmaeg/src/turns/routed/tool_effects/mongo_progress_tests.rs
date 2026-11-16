//! #2171 Phase 1: desired progress guarantees, intentionally red on the baseline.
//! Only the provider HTTP server and final platform transport are fixtures.
use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    turns::routed::{
        adapter_test_server::HttpServer, DispatchOutcome, ExecutionPort, HttpExecutionPort,
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
            sweep(h, &id).await;
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
