use super::{
    super::replica_test_fixture::*,
    mongo_progress_fixture::*,
    mongo_rollback_tests::{operator, request},
    *,
};
use crate::{
    replicas::unresolved::{Operator, Request},
    turns::routed::{FrozenTurnBuilder, Journal, Lease},
};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn unknown(h: &Harness, plan: &AdmittedPlan) -> (Journal, Lease) {
    let until = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        for node in &h.nodes {
            if let Ok(mut lease) = node
                .journal()
                .claim(plan, &plan.targets[0], "history-fixture")
                .await
            {
                let journal = node.journal();
                let channel = h.current.channel.lock().unwrap().clone();
                let body = Builder(h.database.keys())
                    .prepare(plan, &plan.targets[0], &channel)
                    .await
                    .unwrap();
                journal
                    .freeze(&mut lease, plan, &plan.targets[0], &body)
                    .await
                    .unwrap();
                journal
                    .transition(&mut lease, Phase::Unknown)
                    .await
                    .unwrap();
                return (journal, lease);
            }
        }
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn abandon(op: &Operator, target: &str) {
    let binding = op
        .inspect()
        .await
        .unwrap()
        .candidates
        .into_iter()
        .find(|c| c.target == target)
        .unwrap()
        .binding;
    let mut req = Request {
        binding,
        request_id: uuid::Uuid::new_v4().to_string(),
        reason: "history fixture".into(),
        plan_digest: None,
    };
    req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
    op.apply(&req, 1).await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn a_full_expired_audit_index_does_not_block_new_abandonment() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.nodes[owner]
                .journal()
                .transition(&mut lease, Phase::Unknown)
                .await
                .unwrap();
            h.nodes[owner].journal().release(&mut lease).await.unwrap();
            h.nodes[0].seed_expired_operator_index(512).await;
            let before = h.nodes[0].operator_index().await;
            let op = operator(h);
            let view = op.inspect().await.unwrap();
            assert!(view.abandonments.is_empty());
            assert!(view.rollback_safe_after.is_none());
            assert_eq!(view.outstanding_quarantined_effects, 0);
            assert_eq!(
                h.nodes[0].operator_index().await.1,
                before.1,
                "list remains read-only"
            );
            let req = request(h).await;
            let receipt = op.apply(&req, 1).await.unwrap();
            let (index, _) = h.nodes[0].operator_index().await;
            assert_eq!(index.audits.len(), 1);
            assert_eq!(index.deadlines.len(), 1);
            assert_eq!(index.rollback_safe_after, Some(receipt.rollback_safe_after));
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn inspection_counts_all_unexpired_quarantines_in_one_effect_history_pass() {
    with_partitioned_harness(|h| {
        Box::pin(async move {
            let plans = [
                h.plan.clone(),
                plan_for_conversation(h, "another-target", "43").await,
            ];
            for plan in &plans {
                let (journal, mut lease) = unknown(h, plan).await;
                journal.release(&mut lease).await.unwrap();
                let mut capability = h.capability();
                capability.issued_at -= 5;
                capability.event = super::super::model::delivery_event(plan, &plan.targets[0]);
                let root = Store(journal.clone())
                    .admit(
                        plan,
                        &plan.targets[0],
                        "queued",
                        &Payload {
                            capability,
                            caller: caller().key_hash,
                            message: message(),
                        },
                    )
                    .await
                    .unwrap();
                journal
                    .enqueue_effect(plan, &plan.targets[0], &root.id)
                    .await
                    .unwrap();
            }
            let op = operator(h);
            for plan in &plans {
                abandon(&op, &plan.targets[0].id).await;
            }
            let scans = Arc::new(AtomicUsize::new(0));
            let counted = scans.clone();
            let node = h
                .database
                .node_with_handler(
                    35,
                    Some(EventHandler::callback(move |event| {
                        if let CommandEvent::Started(event) = event {
                            if event.command_name == "find"
                                && event
                                    .command
                                    .get_document("filter")
                                    .ok()
                                    .and_then(|filter| filter.get_str("replica.kind").ok())
                                    == Some("effect")
                            {
                                counted.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    })),
                )
                .await;
            let reader = Operator::new(node.partitions.clone(), node.plans(), node.journal());
            let list = reader.inspect().await.unwrap();
            assert_eq!(list.abandonments.len(), 2);
            assert_eq!(list.outstanding_quarantined_effects, 2);
            assert_eq!(
                scans.load(Ordering::SeqCst),
                1,
                "one deployment scan, not one per audit"
            );
            drop(reader);
            drop(node);
        })
    })
    .await;
}
