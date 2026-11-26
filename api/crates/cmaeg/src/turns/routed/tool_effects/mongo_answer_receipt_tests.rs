use super::{
    super::{
        replica_test_fixture::*,
        tool_reply_store::{Admission, Kind, ReplyStore},
        AnswerReceiptLookup,
    },
    mongo_progress_fixture::{sweep, with_harness},
    *,
};
use crate::delivery::{durable::receipt_for, intents::DeliveryOutcome};
use serde_json::json;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn accepted_answer_wait_exits_publish_one_authorized_receipt_without_dispatch() {
    for uncertain_peer in [false, true] {
        with_harness(|h| Box::pin(async move {
            for tools in &mut h.tools {
                Arc::get_mut(tools).unwrap().journal.accepted_answer_receipts = true;
            }
            let (owner, parent) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let replies = ReplyStore(journal.clone(), parent.partition.clone());
            let Admission::Claimed(peer) = replies.admit(
                &h.plan, h.target(), Kind::Automatic, "f".repeat(64),
            ).await.unwrap() else { panic!("reserve an independent reply") };
            if uncertain_peer {
                replies.finish(&peer, &receipt_for(DeliveryOutcome::Unknown {
                    reason: "fixture_peer_uncertain".into(),
                })).await.unwrap();
            }
            let before = replies.operator_record(&h.plan, h.target()).await.unwrap();
            let answer = AcceptedAnswerIdentity { response_id: "fixture-response".into(), item_id: "primary".into() };
            let (operation, _) = answer.operation(h.target(), &message()).unwrap();
            let work = model::work_id(&operation);
            let capability = h.capability();
            let tools = h.tools[1].clone();
            let submit = {
                let answer = answer.clone();
                let capability = capability.clone();
                tokio::spawn(async move {
                    tools.reply_delayed_answer(caller(), &capability, answer, message()).await
                })
            };
            let store = Store(journal.clone());
            tokio::time::timeout(Duration::from_secs(5), async {
                while store.read(&work, &h.plan, h.target()).await.unwrap().is_none() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            let conflicting = AcceptedAnswerIdentity { item_id: "other-item".into(), ..answer.clone() };
            let (_, conflict_hash) = conflicting.operation(h.target(), &message()).unwrap();
            assert!(matches!(
                replies.admit_keyed_answer(&h.plan, h.target(),
                    (operation.clone(), conflict_hash, Some(&conflicting))).await,
                Err(ExecutionError::IdempotencyConflict)
            ));
            if !uncertain_peer {
                sweep(h, &work).await;
                let time_id = crate::connections::store::index_id("delayed-lane-admission-v1", &[&work]);
                let (mut timer, version) = journal.read::<serde_json::Value>(&time_id).await.unwrap().unwrap();
                timer["admitted_at"] = json!((journal.clock)() - 31);
                journal.write(
                    &time_id, &h.plan.identity.owner,
                    crate::replicas::delayed_lane::retention(&h.plan, h.target()),
                    &timer, Some(version),
                ).await.unwrap();
            }
            sweep(h, &work).await;
            let submitted = submit.await.unwrap();
            assert!(matches!(submitted, Ok(_) | Err(ExecutionError::Busy)));
            let expected = if uncertain_peer { "delayed_reply_guard_uncertain" } else { "delayed_reply_guard_wait_expired" };
            let repeat = h.tools[0].reply_delayed_answer(caller(), &capability, answer.clone(), message()).await.unwrap();
            assert!(matches!(repeat.delivery, DeliveryOutcome::Rejected { reason } if reason == expected));
            let channel = h.current.channel.lock().unwrap().clone();
            let visible = h.tools[1].accepted_answer_receipt(
                caller(), &channel,
                AnswerReceiptLookup {
                    channel_id: channel.id.clone(),
                    binding_id: h.target().launch.provider.binding_id.clone(),
                    event_id: h.plan.identity.event_id.clone(),
                    response_id: answer.response_id,
                    item_id: answer.item_id,
                },
            ).await.unwrap().unwrap().result;
            let visible = serde_json::to_value(visible).unwrap();
            assert_eq!(visible["dispatch"], "not_dispatched");
            assert_eq!(visible["delivery_id"], operation);
            assert_eq!(before, replies.operator_record(&h.plan, h.target()).await.unwrap());
            assert!(h.rx.try_recv().is_err(), "no platform command may be enqueued");
        })).await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn stream_binding_refuses_conflicting_delayed_intake_and_replays_without_queueing() {
    with_harness(|h| {
        Box::pin(async move {
            Arc::get_mut(&mut h.tools[1])
                .unwrap()
                .journal
                .accepted_answer_receipts = true;
            let (owner, parent) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let replies = ReplyStore(journal.clone(), parent.partition.clone());
            let answer = AcceptedAnswerIdentity {
                response_id: "fixture-response".into(),
                item_id: "primary".into(),
            };
            let (operation, hash) = answer.operation(h.target(), &message()).unwrap();
            let Admission::Claimed(claim) = replies
                .admit_keyed_answer(
                    &h.plan,
                    h.target(),
                    (operation.clone(), hash, Some(&answer)),
                )
                .await
                .unwrap()
            else {
                panic!("stream reserves the shared answer");
            };
            let capability = h.capability();
            let conflicting = AcceptedAnswerIdentity {
                item_id: "other-item".into(),
                ..answer.clone()
            };
            assert!(matches!(
                h.tools[1]
                    .reply_delayed_answer(caller(), &capability, conflicting, message())
                    .await,
                Err(ExecutionError::IdempotencyConflict)
            ));
            let expected = DeliveryOutcome::Rejected {
                reason: "fixture_stream_refused".into(),
            };
            replies
                .finish(&claim, &receipt_for(expected.clone()))
                .await
                .unwrap();
            let replay = h.tools[1]
                .reply_delayed_answer(caller(), &capability, answer, message())
                .await
                .unwrap();
            assert_eq!(replay.delivery, expected);
            assert!(Store(journal)
                .read(&model::work_id(&operation), &h.plan, h.target())
                .await
                .unwrap()
                .is_none());
            assert!(
                h.rx.try_recv().is_err(),
                "neither replay may enqueue a platform command"
            );
        })
    })
    .await;
}
