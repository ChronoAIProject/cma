use super::*;
use crate::delivery::intents::ConfirmedMessagePart;
use cmaeg_core::{ids::MessageRef, seal::MessageRefPayload};
use serde_json::json;

#[path = "split_answer_tests.rs"]
mod split_answer;

fn partial(h: &Harness) -> IntentReceipt {
    let mut receipt = receipt_for(DeliveryOutcome::Unknown {
        reason: "partial_delivery".into(),
    });
    receipt.message_parts = (0..80)
        .map(|index| {
            let payload: MessageRefPayload = serde_json::from_value(json!({
                "channel_id":h.event.channel_id, "event_id":h.event.event_id,
                "consumer":h.event.consumer, "platform_message_id":(800+index).to_string(),
                "route_id":"telegram:77:42"
            }))
            .unwrap();
            ConfirmedMessagePart {
                index,
                message_ref: MessageRef::seal(&payload, &fixture::keys()).unwrap(),
            }
        })
        .collect();
    receipt
}

#[tokio::test]
async fn partial_split_receipt_replays_after_restart_without_new_delivery_and_missing_page_fails_closed(
) {
    let mut h = Harness::new().await;
    let expected = partial(&h);
    let (result, ()) = tokio::join!(
        h.tools.reply_with_idempotency(
            h.caller,
            &h.channel,
            &h.event,
            message(),
            false,
            Some("split")
        ),
        async {
            let reply = command(&mut h.rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(expected.clone()).unwrap();
        }
    );
    assert_eq!(
        storage::bytes(&result.unwrap().unwrap()).unwrap(),
        storage::bytes(&expected).unwrap()
    );
    let (restarted, mut rx) = lane(&h.setup);
    let replay = restarted
        .tool_replies()
        .reply_with_idempotency(
            h.caller,
            &h.channel,
            &h.event,
            message(),
            false,
            Some("split"),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        storage::bytes(&replay).unwrap(),
        storage::bytes(&expected).unwrap()
    );
    assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
    assert!(rx.try_recv().is_err());
    let page = h
        .setup
        .server
        .memory
        .rows
        .lock()
        .unwrap()
        .iter()
        .find_map(|(id, row)| {
            let opened =
                cmaeg_core::seal::open_interaction(&fixture::keys(), id, &row.blob).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&opened.payload).unwrap();
            (body["kind"] == "native_split_text_receipt_page_v1").then(|| id.clone())
        })
        .unwrap();
    h.setup.server.memory.rows.lock().unwrap().remove(&page);
    assert!(restarted
        .tool_replies()
        .reply_with_idempotency(
            h.caller,
            &h.channel,
            &h.event,
            message(),
            false,
            Some("split")
        )
        .await
        .is_err());
    assert_eq!(
        ReplyStore(h.setup.journal().clone(), None)
            .status(&h.setup.plan, h.setup.target())
            .await
            .unwrap(),
        ToolReplyStatus::Unknown
    );
    assert!(rx.try_recv().is_err());
    assert!(h.rx.try_recv().is_err());
}

#[tokio::test]
async fn automatic_partial_split_keeps_receipt_without_acknowledgement_or_resend() {
    check_automatic_partial_split(false).await;
}

#[tokio::test]
async fn automatic_partial_split_recovers_paged_receipt_after_checkpoint_loss() {
    check_automatic_partial_split(true).await;
}

async fn check_automatic_partial_split(lose_checkpoint: bool) {
    let h = Harness::new().await;
    let expected = partial(&h);
    h.setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut lease = accepted(&h.setup).await;
    let (delivery, mut rx) = lane(&h.setup);
    let frame = recovery_frame("resp_1", 0, false);
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(
            &h.setup.plan.identity,
            &h.setup.target().id,
            &mut lease,
            &frame,
            effect()
        ),
        async {
            let reply = command(&mut rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            if lose_checkpoint {
                h.setup
                    .server
                    .fault(h.setup.target().id.clone(), false, true);
            }
            reply.receipt.send(expected.clone()).unwrap();
        }
    );
    if lose_checkpoint {
        assert!(result.is_err());
        assert!(matches!(
            stored(&h.setup).await.delivery.unwrap().status,
            DeliveryStatus::Intent
        ));
    } else {
        assert!(matches!(result.unwrap(), FrameDelivery::Unknown));
    }
    assert_eq!(cursor(&lease.state), None);
    h.setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut lease = accepted(&h.setup).await;
    let (restarted, mut restarted_rx) = lane(&h.setup);
    assert!(matches!(
        restarted
            .deliver_frame(
                &h.setup.plan.identity,
                &h.setup.target().id,
                &mut lease,
                &frame,
                effect()
            )
            .await
            .unwrap(),
        FrameDelivery::Unknown
    ));
    assert_eq!(cursor(&lease.state), None);
    let state = stored(&h.setup).await;
    let DeliveryStatus::Partial { receipt } = &state.delivery.as_ref().unwrap().status else {
        panic!()
    };
    let checkpoint = state.delivery.as_ref().unwrap();
    let operation = storage::hash(
        &storage::bytes(&(
            &state.id,
            &checkpoint.response_id,
            checkpoint.cursor,
            &checkpoint.frame_hash,
            &checkpoint.effect_hash,
        ))
        .unwrap(),
    );
    let loaded = h
        .setup
        .journal()
        .receipts(&operation, &state.owner, state.retained_until)
        .load(receipt)
        .await
        .unwrap();
    assert_eq!(
        storage::bytes(&loaded).unwrap(),
        storage::bytes(&expected).unwrap()
    );
    assert!(!state.delivery.as_ref().unwrap().status.settled());
    assert!(rx.try_recv().is_err());
    assert!(restarted_rx.try_recv().is_err());
}
