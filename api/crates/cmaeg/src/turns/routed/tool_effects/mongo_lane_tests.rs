use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    replicas::delayed_lane,
    turns::routed::tool_reply_store::{Admission, Kind, ReplyStore},
};
use std::sync::atomic::Ordering;

pub(super) async fn pending(
    h: &Harness,
    key: &str,
) -> (cmaeg_core::seal::ReplyTargetPayload, String) {
    let mut capability = h.capability();
    capability.issued_at -= 5;
    assert!(matches!(
        h.tools[0]
            .reply_delayed(caller(), &capability, key, message())
            .await,
        Err(ExecutionError::Busy)
    ));
    let id = effect_id(h, &capability, key);
    (capability, id)
}
pub(super) async fn claim(
    h: &Harness,
    work: &str,
) -> (usize, crate::replicas::partition::PartitionLease) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for (index, node) in h.nodes.iter().enumerate() {
                if let Ok(lease) = node.claim_effect(&h.plan, h.target(), work).await {
                    return (index, lease);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn lane_waits_for_peer_receipt_then_sends_one_new_intended_message() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            let replies = ReplyStore(h.nodes[owner].journal(), parent.partition.clone());
            let Admission::Claimed(peer) = replies
                .admit(&h.plan, h.target(), Kind::Automatic, "e".repeat(64))
                .await
                .unwrap()
            else {
                panic!("peer")
            };
            let (capability, id) = pending(h, "later-callback").await;
            sweep(h, &id).await;
            assert!(h.rx.try_recv().is_err());
            assert!(matches!(
                h.tools[1]
                    .reply_delayed(caller(), &capability, "later-callback", message())
                    .await,
                Err(ExecutionError::Busy)
            ));
            replies.finish(&peer, &receipt()).await.unwrap();
            h.nodes[owner].journal().release(&mut parent).await.unwrap();
            let (sink, sends) = transport(h, Duration::ZERO);
            sweep(h, &id).await;
            let result = h.tools[1]
                .reply_delayed(caller(), &capability, "later-callback", message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            assert_eq!(sends.load(Ordering::SeqCst), 1);
            sink.abort();
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn expired_peer_attempt_yields_no_send_unknown_and_retires_the_lane_item() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let replies = ReplyStore(journal.clone(), parent.partition.clone());
            assert!(matches!(
                replies
                    .admit(&h.plan, h.target(), Kind::Tool, "f".repeat(64))
                    .await
                    .unwrap(),
                Admission::Claimed(_)
            ));
            journal.release(&mut parent).await.unwrap();
            let (capability, id) = pending(h, "peer-expiry").await;
            sweep(h, &id).await;
            assert!(h.rx.try_recv().is_err());
            tokio::time::sleep(Duration::from_secs(31)).await;
            sweep(h, &id).await;
            let result = h.tools[1]
                .reply_delayed(caller(), &capability, "peer-expiry", message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
            assert!(h.nodes[0].custody(&id).await.unwrap().1);
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn bot_father_callback_after_confirmed_automatic_close_sends_once() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            let replies = ReplyStore(h.nodes[owner].journal(), parent.partition.clone());
            let Admission::Claimed(automatic) = replies
                .admit(&h.plan, h.target(), Kind::Automatic, "a".repeat(64))
                .await
                .unwrap()
            else {
                panic!("auto")
            };
            replies.finish(&automatic, &receipt()).await.unwrap();
            h.finish_parent(owner, &mut parent).await;
            let after_event = h.event().expires_at + 1;
            for tools in &mut h.tools {
                Arc::get_mut(tools).unwrap().journal.clock = Arc::new(move || after_event);
            }

            let (capability, id) = pending(h, "oauth-callback-result").await;
            let (sink, sends) = transport(h, Duration::ZERO);
            sweep(h, &id).await;
            for _ in 0..2 {
                let result = h.tools[1]
                    .reply_delayed(caller(), &capability, "oauth-callback-result", message())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            }
            assert_eq!(sends.load(Ordering::SeqCst), 1);
            sink.abort();
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn cohort_changes_pin_existing_lane_and_legacy_fifo_custody() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            h.nodes[1].lane_capability(false).await;
            let (capability, legacy_id) = pending(h, "legacy").await;
            let legacy = h.nodes[0].custody(&legacy_id).await.unwrap();
            assert_ne!(
                legacy.0,
                delayed_lane::id(
                    &h.nodes[0].partitions.membership().deployment,
                    &h.plan,
                    h.target()
                )
            );
            h.nodes[1].lane_capability(true).await;
            let (_, lane_id) = pending(h, "lane").await;
            let lane = h.nodes[0].custody(&lane_id).await.unwrap();
            assert_ne!(lane.0, legacy.0);
            h.nodes[1].lane_capability(false).await;
            assert!(matches!(
                h.tools[1]
                    .reply_delayed(caller(), &capability, "lane", message())
                    .await,
                Err(ExecutionError::Busy)
            ));
            assert_eq!(h.nodes[0].custody(&lane_id).await.unwrap().0, lane.0);
            h.nodes[1].lane_capability(true).await;
            let (sink, sends) = transport(h, Duration::ZERO);
            super::mongo_tests::execute(h, &legacy_id).await;
            super::mongo_tests::execute(h, &lane_id).await;
            assert_eq!(sends.load(Ordering::SeqCst), 2);
            sink.abort();
            assert_eq!(h.nodes[0].custody(&legacy_id).await.unwrap().0, legacy.0);
            assert_eq!(h.nodes[0].custody(&lane_id).await.unwrap().0, lane.0);
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn simultaneous_fifo_and_lane_admission_pin_one_occurrence_and_send_once() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let mut capability = h.capability();
            capability.issued_at -= 5;
            let payload = Payload {
                capability: capability.clone(),
                caller: caller().key_hash,
                message: message(),
            };
            let store = Store(h.nodes[0].journal());
            let (legacy, selected) = tokio::join!(
                store.admit_legacy(&h.plan, h.target(), "same-key", &payload),
                h.tools[1].reply_delayed(caller(), &capability, "same-key", message())
            );
            let root = legacy.unwrap();
            assert!(matches!(selected, Err(ExecutionError::Busy)));
            h.nodes[0]
                .journal()
                .ensure_delayed_custody(&h.plan, h.target(), &root.id)
                .await
                .unwrap();
            let custody = h.nodes[0].custody(&root.id).await.unwrap().0;
            let (sink, sends) = transport(h, Duration::ZERO);
            super::mongo_tests::execute(h, &root.id).await;
            let result = h.tools[1]
                .reply_delayed(caller(), &capability, "same-key", message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            assert_eq!(sends.load(Ordering::SeqCst), 1);
            sink.abort();
            assert_eq!(h.nodes[0].custody(&root.id).await.unwrap().0, custody);
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn lost_root_receipt_publication_recovers_guard_receipt_without_another_send() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let (capability, id) = pending(h, "lost-terminal").await;
            let (owner, token) = claim(h, &id).await;
            let store = Store(h.nodes[owner].journal());
            let mut root = store.read(&id, &h.plan, h.target()).await.unwrap().unwrap();
            let guard = ReplyStore(h.nodes[owner].journal(), Some(token.clone()));
            let Admission::Claimed(attempt) = guard
                .admit_delayed(
                    &h.plan,
                    h.target(),
                    root.operation.clone(),
                    root.content_hash.clone(),
                )
                .await
                .unwrap()
            else {
                panic!("claim")
            };
            store
                .transition(&mut root, &token, model::State::Sending)
                .await
                .unwrap();
            // Durable crash boundary: external receipt and shared guard are saved,
            // but the delayed root's terminal write was lost. No synthetic re-send.
            guard.finish(&attempt, &receipt()).await.unwrap();
            h.nodes[owner].partitions.release(&token).await.unwrap();
            sweep(h, &id).await;
            let result = h.tools[1]
                .reply_delayed(caller(), &capability, "lost-terminal", message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            assert!(h.nodes[0].custody(&id).await.unwrap().1);
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn lane_expiry_fences_retained_tokens_and_never_recreates_custody() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let before = h.nodes[0].storage_size().await;
            let (capability, id) = pending(h, "expiry").await;
            let after = h.nodes[0].storage_size().await;
            println!(
                "lane_storage one_short_reply_records={} bson_bytes={}",
                after.0 - before.0,
                after.1 - before.1
            );
            let (owner, mut token) = claim(h, &id).await;
            let (lane, _, until) = h.nodes[owner].custody(&id).await.unwrap();
            assert_eq!(until, delayed_lane::retention(&h.plan, h.target()));
            assert!(until > capability.expires_at);
            h.nodes[owner].expire_lane(&lane, &id).await;
            assert!(h.nodes[owner].partitions.renew(&mut token).await.is_err());
            assert!(h.nodes[owner]
                .journal()
                .write_fenced(
                    &Some(token.clone()),
                    &id,
                    &h.plan.identity.owner,
                    h.plan.retained_until,
                    &serde_json::json!({"invalid":"zombie"}),
                    None
                )
                .await
                .is_err());
            assert!(h.nodes[owner]
                .claim_effect(&h.plan, h.target(), &id)
                .await
                .is_err());
            assert!(
                h.nodes[owner]
                    .journal()
                    .ensure_delayed_custody(&h.plan, h.target(), &id)
                    .await
                    .is_err(),
                "expired lane custody cannot become a fresh FIFO item"
            );
            Arc::get_mut(&mut h.tools[0]).unwrap().journal.clock = Arc::new(move || until + 1);
            assert!(h.tools[0]
                .reply_delayed(caller(), &capability, "expiry", message())
                .await
                .is_err());
            assert!(h.nodes[0].custody(&id).await.is_none());
            assert!(
                Store(h.nodes[1].journal())
                    .read(&id, &h.plan, h.target())
                    .await
                    .unwrap()
                    .is_some(),
                "root retains audit identity beyond lane TTL"
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn ordinary_close_races_lane_guard_admission_without_reopening_or_losing_receipt() {
    with_harness(|h| {
        Box::pin(async move {
            let (parent_owner, parent) = h.accepted().await;
            let (_, id) = pending(h, "close-race").await;
            let (owner, token) = claim(h, &id).await;
            let store = Store(h.nodes[owner].journal());
            let root = store.read(&id, &h.plan, h.target()).await.unwrap().unwrap();
            let original = ReplyStore(h.nodes[parent_owner].journal(), parent.partition.clone());
            let delayed = ReplyStore(h.nodes[owner].journal(), Some(token.clone()));
            let (_closed, admitted) = tokio::join!(
                original.close(&h.plan, h.target()),
                delayed.admit_delayed(
                    &h.plan,
                    h.target(),
                    root.operation.clone(),
                    root.content_hash.clone()
                )
            );
            let admitted = match admitted {
                Err(ExecutionError::Busy) => {
                    delayed
                        .admit_delayed(
                            &h.plan,
                            h.target(),
                            root.operation.clone(),
                            root.content_hash.clone(),
                        )
                        .await
                }
                result => result,
            }
            .unwrap();
            let Admission::Claimed(attempt) = admitted else {
                panic!("distinct delayed claim wins or retries");
            };
            delayed.finish(&attempt, &receipt()).await.unwrap();
            original.close(&h.plan, h.target()).await.unwrap();
            let (guard, _) = original.operator_record(&h.plan, h.target()).await.unwrap();
            assert_eq!(guard["closed"], true);
            h.nodes[owner].partitions.release(&token).await.unwrap();
            sweep(h, &id).await;
            assert!(matches!(
                store
                    .receipt(&store.read(&id, &h.plan, h.target()).await.unwrap().unwrap())
                    .await
                    .unwrap()
                    .unwrap()
                    .delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            assert!(
                h.rx.try_recv().is_err(),
                "saved guard receipt must be replayed, not sent twice"
            );
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn cohort_degradation_during_send_preserves_custody_and_guard_consistent_fifo_fallback() {
    use crate::turns::routed::delivery_tests::command;
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let (capability, id) = pending(h, "in-flight-lane").await;
            let lane = h.nodes[0].custody(&id).await.unwrap().0;
            let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
            let ((), fifo_id) = tokio::join!(sweep(h, &id), async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                h.nodes[1].lane_capability(false).await;
                assert!(matches!(
                    h.tools[1]
                        .reply_delayed(caller(), &capability, "in-flight-lane", message())
                        .await,
                    Err(ExecutionError::Busy)
                ));
                let (_, fifo) = pending(h, "new-fifo-intent").await;
                assert_ne!(h.nodes[0].custody(&fifo).await.unwrap().0, lane);
                sweep(h, &fifo).await;
                assert!(
                    rx.try_recv().is_err(),
                    "fallback must wait for the same guard"
                );
                assert!(reply.gate.commit());
                reply.receipt.send(receipt()).unwrap();
                fifo
            });
            assert_eq!(h.nodes[0].custody(&id).await.unwrap().0, lane);
            assert!(matches!(
                h.tools[1]
                    .reply_delayed(caller(), &capability, "in-flight-lane", message())
                    .await
                    .unwrap()
                    .unwrap()
                    .delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            h.nodes[1].lane_capability(true).await;
            let ((), ()) = tokio::join!(super::mongo_tests::execute(h, &fifo_id), async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                reply.receipt.send(receipt()).unwrap();
            });
            assert!(rx.try_recv().is_err());
        })
    })
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn quarantine_cannot_race_live_lane_io_and_refuses_every_later_lane_item() {
    use crate::{
        replicas::unresolved::{Operator, Request},
        turns::routed::delivery_tests::command,
    };
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.nodes[owner]
                .journal()
                .transition(&mut parent, Phase::Unknown)
                .await
                .unwrap();
            h.nodes[owner].journal().release(&mut parent).await.unwrap();
            let (capability, id) = pending(h, "in-flight").await;
            let (_, queued) = pending(h, "queued-next").await;
            let op = Operator::new(
                h.nodes[0].partitions.clone(),
                h.nodes[0].plans(),
                h.nodes[0].journal(),
            );
            let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
            let ((), ()) = tokio::join!(sweep(h, &id), async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                let binding = h.nodes[0].head_binding(&h.plan, h.target()).await;
                let mut req = Request {
                    binding,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    reason: "live lane refusal".into(),
                    plan_digest: None,
                };
                req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
                assert_eq!(op.apply(&req, 1).await.err().unwrap().code(), "live_lease");
                reply.receipt.send(receipt()).unwrap();
            });
            let binding = h.nodes[0].head_binding(&h.plan, h.target()).await;
            let mut req = Request {
                binding,
                request_id: uuid::Uuid::new_v4().to_string(),
                reason: "reviewed target release".into(),
                plan_digest: None,
            };
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            op.apply(&req, 1).await.unwrap();
            sweep(h, &queued).await;
            assert!(h.nodes[0].custody(&queued).await.unwrap().1);
            assert!(matches!(
                h.tools[0]
                    .reply_delayed(caller(), &capability, "fresh-key", message())
                    .await,
                Err(ExecutionError::Abandoned)
            ));
            assert_eq!(op.inspect().await.unwrap().outstanding_lane_items, 0);
            assert!(rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn completed_delayed_reply_storage_is_bounded_and_receipt_outlives_lane_custody() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.nodes[owner].journal().release(&mut parent).await.unwrap();
            let before = h.nodes[0].storage_size().await;
            let (capability, id) = pending(h, "storage-receipt").await;
            let lane = h.nodes[0].custody(&id).await.unwrap().0;
            let (sink, sends) = transport(h, Duration::ZERO);
            sweep(h, &id).await;
            sink.abort();
            assert_eq!(sends.load(Ordering::SeqCst), 1);
            let after = h.nodes[0].storage_size().await;
            println!(
                "lane_storage completed_short_reply_records={} bson_bytes={}",
                after.0 - before.0,
                after.1 - before.1
            );
            assert_eq!(
                after.0 - before.0, 8,
                "one admission slot, payload chunk, root/item/time, shared head/guard, keyed receipt"
            );
            h.nodes[0].expire_lane(&lane, &id).await;
            let stored = Store(h.nodes[1].journal());
            let root = stored
                .read(&id, &h.plan, h.target())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                stored.receipt(&root).await.unwrap().unwrap().delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            let after_expiry = capability.expires_at + 86401;
            Arc::get_mut(&mut h.tools[1]).unwrap().journal.clock = Arc::new(move || after_expiry);
            assert!(matches!(
                h.tools[1]
                    .reply_delayed(caller(), &capability, "storage-receipt", message())
                    .await,
                Err(ExecutionError::Expired)
            ));
            assert!(h.nodes[0].custody(&id).await.is_none());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_discovery_cannot_schedule_or_requeue_a_pinned_expiring_lane() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let (_, id) = pending(h, "rollback-reader").await;
            assert!(
                h.nodes[0].legacy_partition_work().await.is_empty(),
                "PR A partition discovery reads only the permanent store"
            );
            assert!(
                h.nodes[0]
                    .journal()
                    .effect_roots(None, 32)
                    .await
                    .unwrap()
                    .is_empty(),
                "legacy root repair cannot see the lane root"
            );
            let pinned = h.nodes[0].custody(&id).await.unwrap();
            assert_eq!(
                h.nodes[1]
                    .journal()
                    .enqueue_effect(&h.plan, h.target(), &id)
                    .await,
                Err(ExecutionError::Corrupt),
                "old same-key FIFO enqueue cannot adopt the lane item"
            );
            assert_eq!(h.nodes[0].custody(&id).await.unwrap(), pinned);
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn frontier_discovers_lane_behind_unknown_parent_without_isolating_it() {
    use crate::turns::routed::scheduler::RecoverySource;
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            journal
                .transition(&mut parent, Phase::Unknown)
                .await
                .unwrap();
            journal.release(&mut parent).await.unwrap();
            let (capability, work) = pending(h, "frontier-child").await;
            let mut hints = Vec::new();
            for node in &h.nodes {
                let frontier = crate::replicas::partition_discovery::Frontier {
                    partitions: node.partitions.clone(),
                    plans: node.plans(),
                };
                let page = frontier.page(None).await.unwrap();
                assert!(
                    page.issues.is_empty(),
                    "a genuine lane must not be isolated"
                );
                hints.extend(
                    page.plans
                        .into_iter()
                        .flat_map(|p| p.targets)
                        .filter(|t| t.id == work),
                );
            }
            assert_eq!(
                hints.len(),
                1,
                "exactly one capable member discovers the lane"
            );
            let (sink, sends) = transport(h, Duration::ZERO);
            sweep(h, &hints[0].id).await;
            let result = h.tools[1]
                .reply_delayed(caller(), &capability, "frontier-child", message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            assert_eq!(sends.load(Ordering::SeqCst), 1);
            let (state, _) = journal
                .read::<TargetState>(&h.target().id)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(state.phase, Phase::Unknown));
            assert_eq!(
                h.nodes[0].head_binding(&h.plan, h.target()).await.head_work,
                h.target().id
            );
            sink.abort();
        })
    })
    .await;
}
