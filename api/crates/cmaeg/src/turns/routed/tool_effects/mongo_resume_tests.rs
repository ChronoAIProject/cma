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
