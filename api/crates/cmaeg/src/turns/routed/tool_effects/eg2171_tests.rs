//! #2171: an unresolved parent must not deadlock its own delayed reply.
use super::{super::replica_test_fixture::*, mongo_tests::execute, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    turns::routed::{delivery_tests::command, scheduler::RecoverySource, test_support, FrozenTurn},
};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

async fn unknown_parent(h: &Harness) {
    let (owner, mut lease) = h.acquire().await;
    let journal = h.nodes[owner].journal();
    let channel = h.current.channel.lock().unwrap().clone();
    let mut body = test_support::frozen(&h.plan, h.target(), &channel, 32);
    body.event_context.event_ref =
        FrozenTurn::event_ref(&h.plan, h.target(), &h.database.keys()).unwrap();
    journal
        .freeze(&mut lease, &h.plan, h.target(), &body)
        .await
        .unwrap();
    journal
        .transition(&mut lease, Phase::Starting)
        .await
        .unwrap();
    journal
        .transition(&mut lease, Phase::Unknown)
        .await
        .unwrap();
    journal.release(&mut lease).await.unwrap();
}

async fn assert_later_blocked(h: &Harness, later: &AdmittedPlan) {
    for node in &h.nodes {
        assert!(
            matches!(
                node.partitions.claim(later, &later.targets[0]).await,
                Err(crate::replicas::Error::Busy | crate::replicas::Error::Lost)
            ),
            "a delayed continuation must not release an unrelated FIFO successor"
        );
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn unknown_parent_delayed_reply_progresses_once_without_releasing_unrelated_work() {
    let mut h = Harness::new().await;
    let result = AssertUnwindSafe(async {
        unknown_parent(&h).await;
        let mut later = h.plan.clone();
        later.identity.event_id = cmaeg_core::ids::EventId::new("eg2171-unrelated").unwrap();
        later.admission_id = later.identity.admission_id();
        later.targets[0].id = uuid::Uuid::new_v4().to_string();
        h.nodes[1].partitions.enqueue(&later).await.unwrap();
        assert_later_blocked(&h, &later).await;

        let capability = h.capability();
        // Both callers use the public admission path. A pending response must
        // preserve the same operation; retrying it is polling, not a new send.
        for tools in &h.tools {
            assert!(matches!(
                tools
                    .reply_delayed(caller(), &capability, "eg2171-reply", message())
                    .await,
                Err(ExecutionError::Busy)
            ));
        }
        let payload = Payload {
            capability: capability.clone(),
            caller: caller().key_hash,
            message: message(),
        };
        let store = Store(h.nodes[0].journal());
        let root = store
            .admit(&h.plan, h.target(), "eg2171-reply", &payload)
            .await
            .unwrap();
        assert_eq!(
            h.nodes[1]
                .journal()
                .effect_roots(None, 32)
                .await
                .unwrap()
                .len(),
            1
        );
        let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
        let progress = tokio::time::timeout(Duration::from_secs(25), async {
            // Use the durable scheduler frontier, not a manually guessed child
            // ID, to prove that ordinary recovery can discover the continuation.
            loop {
                let mut discovered = false;
                for node in &h.nodes {
                    let frontier = crate::replicas::partition_discovery::Frontier {
                        partitions: node.partitions.clone(),
                        plans: node.plans(),
                    };
                    let page = frontier.page(None).await.unwrap();
                    assert!(page.issues.is_empty());
                    assert!(page
                        .plans
                        .iter()
                        .flat_map(|p| &p.targets)
                        .all(|t| t.id != later.targets[0].id));
                    discovered |= page
                        .plans
                        .iter()
                        .flat_map(|p| &p.targets)
                        .any(|t| t.id == root.id);
                }
                if discovered {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            tokio::join!(execute(&h, &root.id), async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                reply.receipt.send(receipt()).unwrap();
            });
        })
        .await;
        assert!(
            progress.is_ok(),
            "unknown parent deadlocked its delayed continuation"
        );

        assert_later_blocked(&h, &later).await;
        let (parent, _) = h.nodes[1]
            .journal()
            .read::<TargetState>(&h.target().id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parent.phase,
            Phase::Unknown,
            "delivery is not proof of acceptance"
        );
        for tools in &h.tools {
            let replay = tools
                .reply_delayed(caller(), &capability, "eg2171-reply", message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(replay.delivery, DeliveryOutcome::Confirmed { .. }));
        }
        assert!(rx.try_recv().is_err(), "same-key polling duplicated a send");
        let mut changed = message();
        changed.text = Some("different fixture answer".into());
        assert!(matches!(
            h.tools[1]
                .reply_delayed(caller(), &capability, "eg2171-reply", changed)
                .await,
            Err(ExecutionError::IdempotencyConflict)
        ));
    })
    .catch_unwind()
    .await;
    h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn expired_delayed_capability_cannot_bypass_unknown_parent() {
    let mut h = Harness::new().await;
    let result = AssertUnwindSafe(async {
        unknown_parent(&h).await;
        let mut capability = h.capability();
        capability.issued_at -= 60;
        capability.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() - 1;
        for tools in &h.tools {
            assert!(tools
                .reply_delayed(caller(), &capability, "eg2171-expired", message())
                .await
                .is_err());
        }
        assert!(h.nodes[0]
            .journal()
            .effect_roots(None, 32)
            .await
            .unwrap()
            .is_empty());
        assert!(h.rx.try_recv().is_err());
    })
    .catch_unwind()
    .await;
    h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
