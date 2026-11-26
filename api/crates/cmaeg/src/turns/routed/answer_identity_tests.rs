use super::*;
use crate::{
    auth::AgentCaller,
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
    turns::routed::{
        delivery_tests::{accepted, effect, lane},
        test_support::{fixture, recovery_frame, supported, Setup},
        AnswerReceiptLookup, FrameDelivery,
    },
};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn conflicting_first_bindings_cannot_make_the_winning_answer_unreadable() {
    let setup = Setup::new(supported()).await;
    let target = setup.target();
    let store = ReplyStore(setup.journal().clone(), None);
    let stream = AcceptedAnswerIdentity {
        response_id: "resp_1".into(),
        item_id: "stream_item".into(),
    };
    let delayed = AcceptedAnswerIdentity {
        item_id: "different_item".into(),
        ..stream.clone()
    };
    let ReplayEffect::Reply(message) = effect() else {
        unreachable!()
    };
    let (operation, stream_digest) = stream.operation(target, &message).unwrap();
    let (_, delayed_digest) = delayed.operation(target, &message).unwrap();
    let (prepare, refusal) = tokio::join!(
        store.admit_keyed_answer(
            &setup.plan,
            target,
            (operation.clone(), stream_digest.clone(), Some(&stream))
        ),
        store.refuse_delayed_answer(
            &setup.plan,
            target,
            &operation,
            &delayed_digest,
            &delayed,
            "reply_operation_limit",
        ),
    );
    let (winner, digest, expected) = match (prepare, refusal) {
        (Ok(Admission::Claimed(claim)), Err(ExecutionError::IdempotencyConflict)) => {
            let expected = DeliveryOutcome::Confirmed {
                platform_message_id: Some("fixture-message".into()),
            };
            store
                .finish(&claim, &receipt_for(expected.clone()))
                .await
                .unwrap();
            (stream, stream_digest, expected)
        }
        (Err(ExecutionError::IdempotencyConflict), Ok(receipt)) => {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
            (delayed, delayed_digest, receipt.delivery)
        }
        _ => panic!("one binding must win and the other must conflict"),
    };
    let record = store.load(&setup.plan, target).await.unwrap();
    assert!(
        super::super::tool_operation::OperationStore(setup.journal().clone())
            .answer(&record, &winner)
            .await
            .unwrap()
            .is_some()
    );
    let Admission::Replay(receipt) = store
        .admit_delayed_answer(&setup.plan, target, operation, digest, &winner)
        .await
        .unwrap()
    else {
        panic!("winner must remain replayable")
    };
    assert_eq!(receipt.delivery, expected);
}

#[tokio::test]
async fn interrupted_prepare_preserves_dispatch_evidence_in_both_alias_orders() {
    for (frame_first, prepare_wrote) in [(false, false), (true, false), (false, true), (true, true)]
    {
        let setup = Setup::with_request(
            supported(),
            fixture::request(
                |value| value["routing"]["default_targets"] = json!(["default"]),
                fixture::message("fixture"),
            ),
        )
        .await;
        let target = setup.target();
        let mut lease = accepted(&setup).await;
        let (mut delivery, mut sends) = lane(&setup);
        delivery.journal.accepted_answer_receipts = true;
        let answer = AcceptedAnswerIdentity {
            response_id: "resp_1".into(),
            item_id: "primary_item".into(),
        };
        let ReplayEffect::Reply(message) = effect() else {
            unreachable!()
        };
        let outbound = message.text.as_deref().unwrap();
        let (operation, digest) = answer.operation(target, &message).unwrap();
        let mut frame = recovery_frame("resp_1", 0, false);
        frame.event = ResponsesEvent::OutputItemDone {
            index: 0,
            item: OutputItem::Message {
                id: answer.item_id.clone(),
                text_so_far: outbound.into(),
            },
        };
        frame.raw = json!({
            "type":"response.output_item.done", "response_id":"resp_1", "sequence_number":0,
            "output_index":0, "item":{"id":answer.item_id,"type":"message","role":"assistant",
                "cmaeg_delivery":"primary_v1","content":[{"type":"output_text","text":outbound}]}
        });
        let store = ReplyStore(setup.journal().clone(), None);
        let late = || {
            store.admit_delayed_answer(
                &setup.plan,
                target,
                operation.clone(),
                digest.clone(),
                &answer,
            )
        };
        let version_id =
            storage::identifier(&("ordinary_route_tool_operation_v1", &target.id, &operation));
        setup.server.fault(version_id, prepare_wrote, prepare_wrote);
        if frame_first {
            assert!(delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &target.id,
                    &mut lease,
                    &frame,
                    effect()
                )
                .await
                .is_err());
        } else {
            assert!(late().await.is_err());
        }
        assert!(matches!(late().await.unwrap(), Admission::Pending));
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
        let mut successor = accepted(&setup).await;
        if frame_first {
            assert!(matches!(late().await.unwrap(), Admission::Replay(_)));
        }
        let delivery_result = delivery
            .deliver_frame(
                &setup.plan.identity,
                &target.id,
                &mut successor,
                &frame,
                effect(),
            )
            .await
            .unwrap();
        if prepare_wrote {
            assert!(matches!(delivery_result, FrameDelivery::Unknown));
        } else {
            assert!(matches!(delivery_result, FrameDelivery::Settled { .. }));
        }
        assert!(matches!(late().await.unwrap(), Admission::Replay(_)));
        let channel = setup.current.channel.lock().unwrap().clone();
        let result = delivery
            .tool_replies()
            .accepted_answer_receipt(
                AgentCaller {
                    key_hash: channel.agent_key_hash,
                },
                &channel,
                AnswerReceiptLookup {
                    channel_id: channel.id.clone(),
                    binding_id: target.launch.provider.binding_id.clone(),
                    event_id: setup.plan.identity.event_id.clone(),
                    response_id: answer.response_id.clone(),
                    item_id: answer.item_id.clone(),
                },
            )
            .await
            .unwrap()
            .unwrap()
            .result;
        let value = serde_json::to_value(result).unwrap();
        if prepare_wrote {
            assert_eq!(value["dispatch"], "indeterminate");
            assert_eq!(value["outcome"], "unknown");
        } else {
            assert_eq!(value["dispatch"], "not_dispatched");
            assert_eq!(value["outcome"], "rejected");
        }
        assert_eq!(value["delivery_id"], operation);
        assert!(matches!(
            sends.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}

#[tokio::test]
async fn delayed_refusal_is_terminal_for_both_aliases_before_reservation() {
    let setup = Setup::with_request(
        supported(),
        fixture::request(
            |value| value["routing"]["default_targets"] = json!(["default"]),
            fixture::message("fixture"),
        ),
    )
    .await;
    let target = setup.target();
    let store = ReplyStore(setup.journal().clone(), None);
    let answer = AcceptedAnswerIdentity {
        response_id: "resp_1".into(),
        item_id: "primary_item".into(),
    };
    let ReplayEffect::Reply(message) = effect() else {
        unreachable!()
    };
    let (operation, digest) = answer.operation(target, &message).unwrap();
    let no_send = receipt_for(DeliveryOutcome::Rejected {
        reason: "fixture_refusal".into(),
    });
    for index in 0..64_u8 {
        let key = storage::hash(&[index]);
        let Admission::Claimed(claim) = store
            .admit_keyed(&setup.plan, target, key.clone(), key)
            .await
            .unwrap()
        else {
            panic!("fixture must fill all ordinary keys")
        };
        store.finish(&claim, &no_send).await.unwrap();
    }
    assert!(matches!(
        store
            .admit_delayed_answer(
                &setup.plan,
                target,
                operation.clone(),
                digest.clone(),
                &answer
            )
            .await,
        Err(ExecutionError::OperationLimit)
    ));
    for refusal in ["reply_operation_limit", "delayed_reply_authority_refused"] {
        let receipt = store
            .refuse_delayed_answer(&setup.plan, target, &operation, &digest, &answer, refusal)
            .await
            .unwrap();
        assert_eq!(
            receipt.delivery,
            DeliveryOutcome::Rejected {
                reason: "reply_operation_limit".into()
            }
        );
    }
    let frame = FrameOperation {
        id: operation.clone(),
        payload: Some((digest.clone(), answer.clone())),
    };
    let stream = frame.admit(&store, &setup.plan, target).await.unwrap();
    let late = store
        .admit_delayed_answer(&setup.plan, target, operation.clone(), digest, &answer)
        .await
        .unwrap();
    for retry in [stream, late] {
        assert!(
            matches!(retry, Admission::Replay(receipt) if matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }))
        );
    }
    let record = store.load(&setup.plan, target).await.unwrap();
    assert_eq!(record.operation_keys.len(), 64);
    let (delivery, mut sends) = lane(&setup);
    let channel = setup.current.channel.lock().unwrap().clone();
    let visible = delivery
        .tool_replies()
        .accepted_answer_receipt(
            AgentCaller {
                key_hash: channel.agent_key_hash,
            },
            &channel,
            AnswerReceiptLookup {
                channel_id: channel.id.clone(),
                binding_id: target.launch.provider.binding_id.clone(),
                event_id: setup.plan.identity.event_id.clone(),
                response_id: answer.response_id.clone(),
                item_id: answer.item_id.clone(),
            },
        )
        .await
        .unwrap()
        .unwrap()
        .result;
    let visible = serde_json::to_value(visible).unwrap();
    assert_eq!(visible["dispatch"], "not_dispatched");
    assert_eq!(visible["delivery_id"], operation);
    assert!(matches!(
        sends.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let changed_response = AcceptedAnswerIdentity {
        response_id: "another_resp".into(),
        ..answer
    };
    let (other_operation, other_digest) = changed_response.operation(target, &message).unwrap();
    assert_eq!(operation, other_operation);
    assert!(matches!(
        store
            .refuse_delayed_answer(
                &setup.plan,
                target,
                &other_operation,
                &other_digest,
                &changed_response,
                "reply_operation_limit",
            )
            .await,
        Err(ExecutionError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn accepted_answer_aliases_share_the_saved_operation_in_both_orders() {
    for frame_first in [false, true] {
        let setup = Setup::new(supported()).await;
        let store = ReplyStore(setup.journal().clone(), None);
        let answer = AcceptedAnswerIdentity {
            response_id: "resp_1".into(),
            item_id: "primary_item".into(),
        };
        let ReplayEffect::Reply(message) = effect() else {
            panic!("expected an outbound reply");
        };
        let key = answer.operation(setup.target(), &message).unwrap();
        let frame = FrameOperation {
            id: key.0.clone(),
            payload: Some((key.1.clone(), answer.clone())),
        };
        let late = || {
            store.admit_delayed_answer(
                &setup.plan,
                setup.target(),
                key.0.clone(),
                key.1.clone(),
                &answer,
            )
        };
        let stream = || frame.admit(&store, &setup.plan, setup.target());
        let first = if frame_first {
            stream().await
        } else {
            late().await
        };
        let Admission::Claimed(claim) = first.unwrap() else {
            panic!("the first alias must own delivery");
        };
        assert!(matches!(
            store
                .refuse_delayed_answer(
                    &setup.plan,
                    setup.target(),
                    &key.0,
                    &key.1,
                    &answer,
                    "delayed_reply_authority_refused",
                )
                .await,
            Err(ExecutionError::Busy)
        ));
        let second = if frame_first {
            late().await
        } else {
            stream().await
        };
        assert!(matches!(second.unwrap(), Admission::Pending));
        let changed = AcceptedAnswerIdentity {
            item_id: "different_item".into(),
            ..answer.clone()
        };
        let changed_key = changed.operation(setup.target(), &message).unwrap();
        assert_eq!(key.0, changed_key.0);
        assert_ne!(key.1, changed_key.1);
        assert!(matches!(
            store
                .admit_keyed_answer(
                    &setup.plan,
                    setup.target(),
                    (changed_key.0, changed_key.1, Some(&changed))
                )
                .await,
            Err(ExecutionError::IdempotencyConflict)
        ));
        let outcome = DeliveryOutcome::Confirmed {
            platform_message_id: Some("fixture-message".into()),
        };
        store
            .finish(&claim, &receipt_for(outcome.clone()))
            .await
            .unwrap();
        assert_eq!(
            store
                .refuse_delayed_answer(
                    &setup.plan,
                    setup.target(),
                    &key.0,
                    &key.1,
                    &answer,
                    "delayed_reply_authority_refused",
                )
                .await
                .unwrap()
                .delivery,
            outcome
        );
        for retry in [late().await, stream().await] {
            let Admission::Replay(receipt) = retry.unwrap() else {
                panic!("both aliases must reconcile to the saved receipt");
            };
            assert_eq!(receipt.delivery, outcome);
        }
    }
}
