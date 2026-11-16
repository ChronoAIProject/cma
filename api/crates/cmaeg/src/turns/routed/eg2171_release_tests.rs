//! Operator release is one audited FIFO change, never a replay authorization.
use super::{release_test_fixture::ReleaseHarness, *};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn eg2171_release_advances_exactly_one_head_and_duplicate_cannot_pop_next() {
    let h = ReleaseHarness::new().await;
    let result = AssertUnwindSafe(async {
        h.expired().await;
        let original = h.state(0).await;
        let request = h.request().await;
        h.assert_blocked(1).await;
        h.assert_blocked(2).await;
        let released = h.release(0, &request).await.unwrap();
        assert_eq!(released.released_head, h.plans[0].targets[0].id);
        assert_eq!(released.operation_id, request.operation_id);
        assert_eq!(released.disposition, "unknown_abandoned");
        let remaining = vec![
            h.plans[1].targets[0].id.clone(),
            h.plans[2].targets[0].id.clone(),
        ];
        assert_eq!(h.queue().await, remaining);
        let abandoned = h.state(0).await;
        assert!(
            matches!(abandoned.phase, Phase::UnknownAbandoned { ref operation_id }
            if operation_id == &request.operation_id)
        );
        assert!(
            abandoned.body == original.body,
            "frozen original request must survive release"
        );
        assert_eq!(abandoned.request_hash, original.request_hash);
        let audit_id = crate::connections::store::index_id(
            "cmaeg-unresolved-release-v1",
            &[&h.partition, &request.expected_head],
        );
        let audit = h.record(&audit_id, false).await;
        assert_eq!(audit.0["original_phase"]["state"], "unknown");
        assert_eq!(audit.0["request_hash"], original.request_hash);
        assert_eq!(audit.0["result"], serde_json::to_value(&released).unwrap());
        let item_id = crate::connections::store::index_id(
            "cmaeg-partition-item-v1",
            &[&request.expected_head],
        );
        assert_eq!(h.record(&item_id, true).await.0["done"], true);
        let head_after = h.record(&h.partition, false).await;
        for node in [1, 0] {
            assert_eq!(h.release(node, &request).await.unwrap(), released);
            assert_eq!(
                h.nodes[node].partitions.enqueue(&h.plans[0]).await,
                Err(crate::replicas::Error::Expired),
                "abandoned work must not be readmitted"
            );
            assert_eq!(h.record(&h.partition, false).await, head_after);
            assert_eq!(h.record(&audit_id, false).await, audit);
        }
        let mut different_operation = request.clone();
        different_operation.operation_id = uuid::Uuid::new_v4().to_string();
        assert!(matches!(
            h.release(1, &different_operation).await,
            Err(ExecutionError::LostLease)
        ));
        assert_eq!(h.queue().await, remaining);
        let (owner, mut successor) = h.claim(1).await;
        h.assert_blocked(2).await;
        h.nodes[owner]
            .journal()
            .release(&mut successor)
            .await
            .unwrap();
        assert_eq!(
            h.queue().await,
            remaining,
            "claim/release must not consume the next head"
        );
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
async fn eg2171_two_replicas_release_same_head_with_one_fenced_winner() {
    let h = ReleaseHarness::new().await;
    let result = AssertUnwindSafe(async {
        h.expired().await;
        let first = h.request().await;
        let mut second = first.clone();
        second.operation_id = uuid::Uuid::new_v4().to_string();
        let (a, b) = tokio::join!(h.release(0, &first), h.release(1, &second));
        assert_eq!(
            usize::from(a.is_ok()) + usize::from(b.is_ok()),
            1,
            "independent replicas must not both release one observed version: {a:?} / {b:?}"
        );
        let (winner, loser, result) = match (a, b) {
            (Ok(result), Err(ExecutionError::Busy | ExecutionError::LostLease)) => {
                (&first, &second, result)
            }
            (Err(ExecutionError::Busy | ExecutionError::LostLease), Ok(result)) => {
                (&second, &first, result)
            }
            other => panic!("release race did not return a fenced loser: {other:?}"),
        };
        assert_eq!(h.release(1, winner).await.unwrap(), result);
        assert!(matches!(
            h.release(0, loser).await,
            Err(ExecutionError::LostLease)
        ));
        assert_eq!(
            h.queue().await,
            vec![
                h.plans[1].targets[0].id.clone(),
                h.plans[2].targets[0].id.clone()
            ]
        );
        assert!(
            matches!(h.state(0).await.phase, Phase::UnknownAbandoned { operation_id }
            if operation_id == winner.operation_id)
        );
        let (owner, mut next) = h.claim(1).await;
        h.assert_blocked(2).await;
        h.nodes[owner].journal().release(&mut next).await.unwrap();
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
async fn eg2171_release_rejects_wrong_scope_stale_proof_and_live_authority() {
    let h = ReleaseHarness::new().await;
    let result = AssertUnwindSafe(async {
        let request = h.request().await;
        assert!(matches!(
            h.release(0, &request).await,
            Err(ExecutionError::Expired)
        ));
        h.expired().await;
        let request = h.request().await;
        let before = h.record(&h.partition, false).await;
        let plan = &h.plans[0];
        let service = h.service(1, true);
        assert!(matches!(
            service
                .release(
                    &plan.identity.channel_id,
                    "different-owner",
                    &"a".repeat(64),
                    &request
                )
                .await,
            Err(ExecutionError::AuthorityChanged | ExecutionError::Corrupt)
        ));
        let wrong_channel = cmaeg_core::ids::ChannelId::new("cmaeg1.ch.different-fixture").unwrap();
        assert!(matches!(
            service
                .release(
                    &wrong_channel,
                    &plan.identity.owner,
                    &"a".repeat(64),
                    &request
                )
                .await,
            Err(ExecutionError::AuthorityChanged)
        ));
        for mutation in 0..5 {
            let mut changed = request.clone();
            match mutation {
                0 => changed.expected_head = h.plans[1].targets[0].id.clone(),
                1 => changed.expected_epoch += 1,
                2 => changed.expected_version += 1,
                3 => changed.provider_terminal_confirmed = false,
                _ => changed.suppress_delayed_replies = false,
            }
            let expected = if mutation < 3 {
                ExecutionError::LostLease
            } else {
                ExecutionError::Invalid
            };
            assert_eq!(
                h.release(0, &changed).await.err(),
                Some(expected),
                "invalid release returned the wrong refusal: {mutation}"
            );
            assert_eq!(h.record(&h.partition, false).await, before);
            assert_eq!(h.state(0).await.phase, Phase::Unknown);
        }
        assert!(matches!(
            h.service(0, false)
                .release(
                    &plan.identity.channel_id,
                    &plan.identity.owner,
                    &"a".repeat(64),
                    &request
                )
                .await,
            Err(ExecutionError::Unsupported)
        ));
        assert_eq!(h.record(&h.partition, false).await, before);
        let (owner, mut live) = h.claim(0).await;
        let request = h.request().await;
        assert!(matches!(
            h.release(1 - owner, &request).await,
            Err(ExecutionError::Busy)
        ));
        h.nodes[owner].journal().guard(&live).await.unwrap();
        assert_eq!(h.state(0).await.phase, Phase::Unknown);
        h.nodes[owner]
            .journal()
            .transition(
                &mut live,
                Phase::Accepted {
                    response_id: "eg2171-running".into(),
                    durable_cursor: None,
                },
            )
            .await
            .unwrap();
        h.nodes[owner].journal().release(&mut live).await.unwrap();
        let request = h.request().await;
        let before_accepted = h.record(&h.partition, false).await;
        assert!(matches!(
            h.release(1 - owner, &request).await,
            Err(ExecutionError::LostLease)
        ));
        assert_eq!(h.record(&h.partition, false).await, before_accepted);
        assert!(matches!(h.state(0).await.phase, Phase::Accepted { .. }));
        assert_eq!(h.queue().await.len(), 3);
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
async fn eg2171_release_quarantines_queued_delayed_reply_without_sending() {
    use crate::{
        delivery::{durable::DeliveryCoordinator, router::ShardedScheduler},
        turns::routed::scheduler::{Disposition, TargetExecutor, WorkItem},
    };
    use futures_util::future::BoxFuture;
    use std::{sync::Arc, time::Duration};
    struct NoParent;
    impl TargetExecutor for NoParent {
        fn execute<'a>(&'a self, _work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
            Box::pin(async { panic!("release must not resubmit the original provider request") })
        }
    }
    let h = ReleaseHarness::with_successors(false).await;
    let result = AssertUnwindSafe(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let tools: Vec<_> = h
            .nodes
            .iter()
            .map(|node| {
                Arc::new(RoutedToolReplies::new(
                    node.plans(),
                    node.journal(),
                    h.current.clone(),
                    DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx.clone()))),
                ))
            })
            .collect();
        let plan = &h.plans[0];
        let target = &plan.targets[0];
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let capability = cmaeg_core::seal::ReplyTargetPayload {
            v: 1,
            provider: target.launch.provider.slug.clone(),
            binding_id: target.launch.provider.binding_id.clone(),
            agent_key_hash: [9; 32],
            issued_at: now,
            expires_at: now + 3600,
            event: model::delivery_event(plan, target),
        };
        assert!(matches!(
            tools[0]
                .reply_delayed(
                    replica_test_fixture::caller(),
                    &capability,
                    "release-quarantine",
                    replica_test_fixture::message()
                )
                .await,
            Err(ExecutionError::Busy)
        ));
        let roots = h.nodes[0].journal().effect_roots(None, 32).await.unwrap();
        assert_eq!(roots.len(), 1);
        let work = WorkItem {
            identity: plan.identity.clone(),
            target_id: roots[0].0.clone(),
        };
        h.expired().await;
        let request = h.request().await;
        h.release(1, &request).await.unwrap();
        assert!(rx.try_recv().is_err());
        // Quarantine is a fenced effect settlement. It must finish with a
        // rejected receipt without ever enqueueing a native delivery command.
        let until = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            let mut settled = false;
            for (node, tools) in h.nodes.iter().zip(&tools) {
                let executor = node
                    .journal()
                    .effect_executor(
                        node.plans(),
                        Arc::new(NoParent),
                        tools.replica_effect_executor(),
                    )
                    .unwrap();
                if matches!(executor.execute(&work).await, Disposition::Settled) {
                    settled = true;
                    break;
                }
            }
            assert!(
                rx.try_recv().is_err(),
                "abandoned parent's delayed reply was sent"
            );
            if settled {
                break;
            }
            assert!(
                tokio::time::Instant::now() < until,
                "quarantine did not settle"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let saved = h.record(&work.target_id, true).await.0;
        assert_eq!(saved["state"]["phase"], "settled");
        let receipt: crate::delivery::retained_receipt::StoredReceipt =
            serde_json::from_value(saved["state"]["receipt"].clone()).unwrap();
        assert!(matches!(
            receipt.delivery,
            crate::delivery::intents::DeliveryOutcome::Rejected { .. }
        ));
        for tools in &tools {
            assert!(matches!(
                tools
                    .reply_delayed(
                        replica_test_fixture::caller(),
                        &capability,
                        "release-quarantine",
                        replica_test_fixture::message()
                    )
                    .await,
                Err(ExecutionError::AuthorityChanged)
            ));
            tools.replica_effect_executor().recover(None).await.unwrap();
        }
        assert!(h.nodes[0]
            .journal()
            .effect_roots(None, 32)
            .await
            .unwrap()
            .is_empty());
        assert!(h.queue().await.is_empty());
        assert!(rx.try_recv().is_err());
    })
    .catch_unwind()
    .await;
    h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
