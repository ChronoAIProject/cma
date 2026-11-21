//! Completion regressions for a renewable peer and pre-lane uncertainty.
use super::{
    super::replica_test_fixture::*,
    mongo_lane_tests::{claim, pending},
    mongo_progress_fixture::*,
    *,
};
use crate::{
    delivery::intents::DeliveryOutcome,
    turns::routed::tool_reply_store::{Admission, Kind, ReplyStore, ToolReplyStatus},
};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn pending_wait_cap_saves_receipt_without_changing_a_live_peer() {
    with_harness(|h| Box::pin(async move {
        let (owner, parent) = h.accepted().await;
        let journal = h.nodes[owner].journal();
        let replies = ReplyStore(journal.clone(), parent.partition.clone());
        let Admission::Claimed(peer) = replies.admit(
            &h.plan, h.target(), Kind::Automatic, "f".repeat(64)
        ).await.unwrap() else { panic!("peer") };
        let (capability, work) = pending(h, "bounded-pending").await;
        sweep(h, &work).await;
        assert!(h.rx.try_recv().is_err());
        // Model a long-lived renewed peer without advancing Mongo's lease clock.
        // Only this occurrence's durable admission age changes.
        let time_id = crate::connections::store::index_id("delayed-lane-admission-v1", &[&work]);
        let (mut time, version) = journal.read::<serde_json::Value>(&time_id).await.unwrap().unwrap();
        time["admitted_at"] = serde_json::json!((journal.clock)() - 31);
        journal.write(&time_id, &h.plan.identity.owner,
            crate::replicas::delayed_lane::retention(&h.plan, h.target()),
            &time, Some(version)).await.unwrap();
        let before = replies.operator_record(&h.plan, h.target()).await.unwrap();
        sweep(h, &work).await;
        let result = h.tools[1].reply_delayed(caller(), &capability, "bounded-pending", message()).await;
        assert!(matches!(result, Ok(Some(ref r)) if matches!(r.delivery,
            DeliveryOutcome::Unknown { ref reason } if reason == "delayed_reply_guard_wait_expired")),
            "bounded wait must produce a receipt: {result:?}");
        assert_eq!(before, replies.operator_record(&h.plan, h.target()).await.unwrap());
        assert!(h.nodes[0].custody(&work).await.unwrap().1);
        replies.finish(&peer, &receipt()).await.unwrap();
        let replay = h.tools[0].reply_delayed(caller(), &capability, "bounded-pending", message()).await.unwrap().unwrap();
        assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(h.rx.try_recv().is_err());
    })).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_unknown_execution_publishes_shared_guard_before_later_lane_send() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let payload = Payload {
                capability: h.capability(),
                caller: caller().key_hash,
                message: message(),
            };
            let store = Store(h.nodes[0].journal());
            let mut root = store
                .admit_legacy(&h.plan, h.target(), "prior-unknown", &payload)
                .await
                .unwrap();
            h.nodes[0]
                .journal()
                .ensure_delayed_custody(&h.plan, h.target(), &root.id)
                .await
                .unwrap();
            let (owner, token) = claim(h, &root.id).await;
            Store(h.nodes[owner].journal())
                .transition(&mut root, &token, model::State::Unknown { receipt: None })
                .await
                .unwrap();
            h.nodes[owner].partitions.release(&token).await.unwrap();
            sweep(h, &root.id).await;
            assert_eq!(
                ReplyStore(h.nodes[0].journal(), None)
                    .status(&h.plan, h.target())
                    .await
                    .unwrap(),
                ToolReplyStatus::Unknown,
                "pre-lane uncertainty must enter shared arbitration"
            );
            let (capability, lane) = pending(h, "later-lane").await;
            sweep(h, &lane).await;
            let result = h.tools[1]
                .reply_delayed(caller(), &capability, "later-lane", message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
            assert!(h.nodes[0].custody(&lane).await.unwrap().1);
            assert!(
                !h.nodes[0].custody(&root.id).await.unwrap().1,
                "legacy uncertainty retains FIFO custody"
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn accepted_parent_delivery_uncertainty_with_empty_guard_does_not_block_callback() {
    use crate::turns::routed::delivery_state::{DeliveryCheckpoint, DeliveryStatus};
    for partial in [false, true] {
        with_harness(|h| Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let Phase::Accepted { response_id, .. } = &parent.state.phase else { panic!("accepted") };
            let mut saved = crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
                reason: "retained_parent_delivery".into(),
            });
            let reference: cmaeg_core::seal::MessageRefPayload = serde_json::from_value(serde_json::json!({
                "channel_id":h.event().channel_id,"event_id":h.event().event_id,
                "consumer":h.event().consumer,"platform_message_id":"800","route_id":"telegram:77:42"
            })).unwrap();
            saved.message_parts.push(crate::delivery::intents::ConfirmedMessagePart {
                index: 0,
                message_ref: cmaeg_core::ids::MessageRef::seal(&reference, &h.database.keys()).unwrap(),
            });
            let mut next = parent.state.clone();
            next.delivery = Some(DeliveryCheckpoint {
                execution_attribution: None,
                trace_contract: false, response_id: response_id.clone(), cursor: 1,
                frame_hash: "a".repeat(64), effect_hash: "b".repeat(64), terminal: true,
                status: if partial { DeliveryStatus::Partial { receipt: saved.into() } } else { DeliveryStatus::Unknown },
                custom: None,
            });
            journal.commit(&mut parent, next).await.unwrap();
            journal.release(&mut parent).await.unwrap();
            assert_eq!(ReplyStore(journal.clone(), None).status(&h.plan, h.target()).await.unwrap(), ToolReplyStatus::None);
            let (capability, work) = pending(h, "independent-answer").await;
            let (sink, sends) = transport(h, Duration::ZERO);
            sweep(h, &work).await;
            let result = h.tools[1].reply_delayed(caller(), &capability, "independent-answer", message()).await.unwrap().unwrap();
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 1);
            let (after, _) = journal.read::<TargetState>(&h.target().id).await.unwrap().unwrap();
            assert!(matches!(after.phase, Phase::Accepted { .. }));
            assert!(!after.delivery.unwrap().status.settled());
            sink.abort();
        })).await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn draining_member_without_lane_capability_keeps_new_admission_in_fifo() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            h.nodes[1].lane_capability(false).await;
            h.nodes[1].control.start();
            let op = crate::replicas::unresolved::Operator::new(
                h.nodes[0].partitions.clone(),
                h.nodes[0].plans(),
                h.nodes[0].journal(),
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if op
                        .inspect()
                        .await
                        .unwrap()
                        .members
                        .iter()
                        .any(|m| m.draining && !m.delayed_lane)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let (_, work) = pending(h, "draining-cohort").await;
            assert_ne!(
                h.nodes[0].custody(&work).await.unwrap().0,
                crate::replicas::delayed_lane::id(
                    &h.nodes[0].partitions.membership().deployment,
                    &h.plan,
                    h.target()
                )
            );
            let (sink, sends) = transport(h, Duration::ZERO);
            super::mongo_tests::execute(h, &work).await;
            assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 1);
            sink.abort();
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_admission_limit_includes_no_send_and_fifo_occurrences_before_payload_storage() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let replies = ReplyStore(journal.clone(), parent.partition.clone());
            let Admission::Claimed(peer) = replies
                .admit(&h.plan, h.target(), Kind::Automatic, "f".repeat(64))
                .await
                .unwrap()
            else {
                panic!("guard")
            };
            replies
                .finish(
                    &peer,
                    &crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
                        reason: "prior_uncertainty".into(),
                    }),
                )
                .await
                .unwrap();
            journal.release(&mut parent).await.unwrap();
            let store = Store(journal.clone());
            let payload = Payload {
                capability: h.capability(),
                caller: caller().key_hash,
                message: message(),
            };
            let first = store
                .admit(&h.plan, h.target(), "occurrence-0", &payload)
                .await
                .unwrap();
            sweep(h, &first.id).await;
            let saved = store
                .read(&first.id, &h.plan, h.target())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                store.receipt(&saved).await.unwrap().unwrap().delivery,
                DeliveryOutcome::Unknown { .. }
            ));
            for index in 1..64 {
                if index == 32 {
                    h.nodes[1].lane_capability(false).await;
                }
                let root = store
                    .admit(
                        &h.plan,
                        h.target(),
                        &format!("occurrence-{index}"),
                        &payload,
                    )
                    .await
                    .unwrap();
                journal
                    .ensure_delayed_custody(&h.plan, h.target(), &root.id)
                    .await
                    .unwrap();
            }
            let before = h.nodes[0].storage_size().await;
            assert!(
                matches!(
                    store
                        .admit(&h.plan, h.target(), "occurrence-64", &payload)
                        .await,
                    Err(ExecutionError::OperationLimit)
                ),
                "65th occurrence must not publish a root or payload"
            );
            assert_eq!(h.nodes[0].storage_size().await, before);
            assert_eq!(
                store
                    .admit(&h.plan, h.target(), "occurrence-0", &payload)
                    .await
                    .unwrap()
                    .id,
                first.id
            );
            let mut changed = payload.clone();
            changed.message.text = Some("different content".into());
            assert!(matches!(
                store
                    .admit(&h.plan, h.target(), "occurrence-0", &changed)
                    .await,
                Err(ExecutionError::IdempotencyConflict)
            ));
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_slot_reconciles_concurrent_same_key_and_interrupted_payload_preparation() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.nodes[owner].journal().release(&mut parent).await.unwrap();
            let payload = Payload {
                capability: h.capability(),
                caller: caller().key_hash,
                message: message(),
            };
            let work = model::work_id(&model::operation(h.target(), &payload, "reserved").unwrap());
            let hash = super::super::storage::hash(
                &super::super::storage::bytes(&payload.message).unwrap(),
            );
            let before = h.nodes[0].storage_size().await;
            let (first, second) = tokio::join!(
                h.nodes[0]
                    .partitions
                    .reserve_delayed(&h.plan, h.target(), &work, &hash),
                h.nodes[1]
                    .partitions
                    .reserve_delayed(&h.plan, h.target(), &work, &hash)
            );
            assert!(matches!(
                first,
                Ok(crate::replicas::DelayedReservation::Ready)
            ));
            assert!(matches!(
                second,
                Ok(crate::replicas::DelayedReservation::Ready)
            ));
            let reserved = h.nodes[0].storage_size().await;
            assert_eq!(
                reserved.0 - before.0,
                1,
                "same key consumes exactly one immutable slot"
            );
            let store = Store(h.nodes[1].journal());
            assert!(store
                .read(&work, &h.plan, h.target())
                .await
                .unwrap()
                .is_none());
            let mut changed = payload.clone();
            changed.message.text = Some("different content".into());
            assert!(matches!(
                store.admit(&h.plan, h.target(), "reserved", &changed).await,
                Err(ExecutionError::IdempotencyConflict)
            ));
            assert_eq!(h.nodes[0].storage_size().await, reserved);
            let root = store
                .admit(&h.plan, h.target(), "reserved", &payload)
                .await
                .unwrap();
            assert_eq!(root.id, work);
            assert_eq!(
                h.nodes[0].storage_size().await.0 - before.0,
                6,
                "retry restores payload and atomic custody without reserving again"
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_slot_last_capacity_race_publishes_only_one_new_occurrence() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.nodes[owner].journal().release(&mut parent).await.unwrap();
            let payload = Payload {
                capability: h.capability(),
                caller: caller().key_hash,
                message: message(),
            };
            let hash = super::super::storage::hash(
                &super::super::storage::bytes(&payload.message).unwrap(),
            );
            for index in 0..63 {
                let work = model::work_id(
                    &model::operation(h.target(), &payload, &format!("reserved-{index}")).unwrap(),
                );
                assert!(matches!(
                    h.nodes[0]
                        .partitions
                        .reserve_delayed(&h.plan, h.target(), &work, &hash)
                        .await,
                    Ok(crate::replicas::DelayedReservation::Ready)
                ));
            }
            let first = Store(h.nodes[0].journal());
            let second = Store(h.nodes[1].journal());
            let (a, b) = tokio::join!(
                first.admit(&h.plan, h.target(), "contender-a", &payload),
                second.admit(&h.plan, h.target(), "contender-b", &payload)
            );
            assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
            assert!(
                matches!(a, Err(ExecutionError::OperationLimit))
                    || matches!(b, Err(ExecutionError::OperationLimit))
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_admission_without_committed_root_never_reports_admitted_pending() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.nodes[owner].journal().release(&mut parent).await.unwrap();
            let capability = h.capability();
            let work = effect_id(h, &capability, "obstructed-root");
            let time = crate::connections::store::index_id("delayed-lane-admission-v1", &[&work]);
            let journal = h.nodes[0].journal();
            // Orphaned custody evidence obstructs the create-only publication. It
            // must never turn missing root custody into HTTP admitted-pending 409.
            journal
                .write(
                    &time,
                    &h.plan.identity.owner,
                    crate::replicas::delayed_lane::retention(&h.plan, h.target()),
                    &serde_json::json!({"kind":"delayed_lane_admission_v1","work":work,
                "target":h.target().id,"admitted_at":(journal.clock)()}),
                    None,
                )
                .await
                .unwrap();
            let result = h.tools[0]
                .reply_delayed(caller(), &capability, "obstructed-root", message())
                .await;
            assert!(
                matches!(
                    result,
                    Err(ExecutionError::Uncertain | ExecutionError::Unavailable)
                ),
                "no committed root cannot mean admitted: {result:?}"
            );
            assert!(Store(journal)
                .read(&work, &h.plan, h.target())
                .await
                .unwrap()
                .is_none());
            assert!(h.nodes[0].custody(&work).await.is_none());
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}
