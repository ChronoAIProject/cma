use super::{
    delivery_tests::*,
    test_support::*,
    tool_reply_store::{Admission, Kind, ReplyStore},
    *,
};
use crate::{
    auth::AgentCaller,
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
};
use std::sync::{atomic::Ordering, Arc};

fn caller(setup: &Setup) -> AgentCaller {
    let mut current = setup.current.channel.lock().unwrap();
    Arc::get_mut(&mut current).unwrap().consumer_keys =
        vec![(setup.target().launch.consumer.clone(), [9; 32])];
    AgentCaller { key_hash: [9; 32] }
}
fn message() -> cmaeg_core::message::OutboundMessage {
    let ReplayEffect::Reply(message) = effect() else {
        unreachable!()
    };
    *message
}
fn receipt() -> crate::delivery::intents::IntentReceipt {
    receipt_for(DeliveryOutcome::Confirmed {
        platform_message_id: Some("tool-sent".into()),
    })
}

#[tokio::test]
async fn confirmed_tool_reply_survives_restart_and_defeats_a_stale_auto_final_decision() {
    let setup = Setup::new(supported()).await;
    let caller = caller(&setup);
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    assert_eq!(
        delivery
            .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body)
            .await
            .unwrap(),
        ToolReplyStatus::None
    );
    let tools = delivery.tool_replies();
    let (result, ()) = tokio::join!(
        tools.reply(caller, &channel, &event, message(), false),
        async {
            let reply = command(&mut rx).await;
            assert_eq!(
                delivery
                    .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body)
                    .await
                    .unwrap(),
                ToolReplyStatus::Pending
            );
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt()).unwrap();
        }
    );
    assert!(matches!(
        result.unwrap().unwrap().delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    setup.journal().release(&mut lease).await.unwrap();
    let mut resumed = accepted(&setup).await;
    assert_eq!(
        delivery
            .tool_reply_status(&setup.plan.identity, &setup.target().id, &resumed, &body)
            .await
            .unwrap(),
        ToolReplyStatus::Confirmed
    );
    // Simulate projection having persisted Reply from a stale pre-tool lookup.
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut resumed,
                &recovery_frame("resp_1", 0, true),
                effect()
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { receipt: None, .. }
    ));
    assert!(rx.try_recv().is_err());
    setup
        .journal()
        .complete(&mut resumed, "resp_1")
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_confirmation_after_a_successful_send_never_implies_an_unsent_reply() {
    for persisted in [false, true] {
        let setup = Setup::new(supported()).await;
        let caller = caller(&setup);
        let mut lease = accepted(&setup).await;
        let body = setup.journal().body(&lease).await.unwrap();
        let event = event(&body);
        let channel = setup.current.channel.lock().unwrap().clone();
        let (delivery, mut rx) = lane(&setup);
        let tools = delivery.tool_replies();
        let (result, ()) = tokio::join!(
            tools.reply(caller, &channel, &event, message(), false),
            async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                let id = super::storage::identifier(&(
                    "ordinary-route-reply-guard-v1",
                    &setup.target().id,
                ));
                setup.server.fault(id, persisted, true);
                reply.receipt.send(receipt()).unwrap();
            }
        );
        assert!(result.is_err());
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
        let mut resumed = accepted(&setup).await;
        let status = delivery
            .tool_reply_status(&setup.plan.identity, &setup.target().id, &resumed, &body)
            .await
            .unwrap();
        assert_eq!(
            status,
            if persisted {
                ToolReplyStatus::Confirmed
            } else {
                ToolReplyStatus::Unknown
            }
        );
        let result = delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut resumed,
                &recovery_frame("resp_1", 0, true),
                effect(),
            )
            .await
            .unwrap();
        if persisted {
            assert!(matches!(
                result,
                FrameDelivery::Settled { receipt: None, .. }
            ));
        } else {
            assert!(matches!(result, FrameDelivery::Unknown));
            assert_eq!(cursor(&resumed.state), None);
        }
        assert!(rx.try_recv().is_err());
        // Repeating the tool call cannot turn an uncertain send into another send.
        let result = tools
            .reply(caller, &channel, &event, message(), false)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            result.delivery,
            DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
        ));
        assert!(rx.try_recv().is_err());
        // Keep the stale owner's lease unused; it cannot overwrite the receipt.
        assert!(setup.journal().release(&mut lease).await.is_err());
    }
}

#[tokio::test]
async fn an_inflight_tool_reply_blocks_auto_final_then_unknown_stays_blocked() {
    let setup = Setup::new(supported()).await;
    let caller = caller(&setup);
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    let tools = delivery.tool_replies();
    let frame = recovery_frame("resp_1", 0, true);
    let (result, ()) = tokio::join!(
        tools.reply(caller, &channel, &event, message(), false),
        async {
            let reply = command(&mut rx).await;
            assert!(matches!(
                delivery
                    .deliver_frame(
                        &setup.plan.identity,
                        &setup.target().id,
                        &mut lease,
                        &frame,
                        effect()
                    )
                    .await,
                Err(ExecutionError::Busy)
            ));
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Unknown {
                    reason: "network_lost".into(),
                }))
                .unwrap();
        }
    );
    assert!(matches!(
        result.unwrap().unwrap().delivery,
        DeliveryOutcome::Unknown { .. }
    ));
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &frame,
                effect()
            )
            .await
            .unwrap(),
        FrameDelivery::Unknown
    ));
    assert_eq!(cursor(&lease.state), None);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn cancelled_tool_intent_does_not_become_confirmed_and_cannot_send_late() {
    let setup = Setup::new(supported()).await;
    let caller = caller(&setup);
    let lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    let tools = delivery.tool_replies();
    let mut pending = Box::pin(tools.reply(caller, &channel, &event, message(), false));
    let reply = tokio::select! { reply = command(&mut rx) => reply, _ = &mut pending => panic!("send pending") };
    drop(pending);
    assert!(!reply.gate.commit());
    assert!(reply.gate.attempt().await.is_err());
    assert_eq!(
        delivery
            .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body)
            .await
            .unwrap(),
        ToolReplyStatus::Pending
    );
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let lease = accepted(&setup).await;
    assert_eq!(
        delivery
            .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body)
            .await
            .unwrap(),
        ToolReplyStatus::Unknown
    );
}

#[tokio::test]
async fn stale_tool_writer_cannot_erase_or_overwrite_a_new_automatic_reservation() {
    let setup = Setup::new(supported()).await;
    let store = ReplyStore(setup.journal().clone());
    let Admission::Claimed(first) = store
        .admit(&setup.plan, setup.target(), Kind::Tool, "a".repeat(64))
        .await
        .unwrap()
    else {
        panic!("claimed")
    };
    store
        .finish(
            &first,
            &receipt_for(DeliveryOutcome::Rejected {
                reason: "not_sent".into(),
            }),
        )
        .await
        .unwrap();
    let Admission::Claimed(next) = store
        .admit(&setup.plan, setup.target(), Kind::Automatic, "b".repeat(64))
        .await
        .unwrap()
    else {
        panic!("claimed")
    };
    assert!(matches!(
        store.finish(&first, &receipt()).await,
        Err(ExecutionError::LostLease)
    ));
    store.guard(&next).await.unwrap();
    assert_eq!(
        store.status(&setup.plan, setup.target()).await.unwrap(),
        ToolReplyStatus::Pending
    );
}

#[tokio::test]
async fn revoked_caller_and_unavailable_status_fail_closed_without_delivery() {
    let setup = Setup::new(supported()).await;
    let caller = caller(&setup);
    let lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    setup.current.revoke_at.store(0, Ordering::SeqCst);
    assert!(matches!(
        delivery
            .tool_replies()
            .reply(caller, &channel, &event, message(), false)
            .await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert!(delivery
        .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body)
        .await
        .is_err());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn terminal_no_effect_waits_for_tool_receipt_before_closing_and_acknowledging() {
    let setup = Setup::new(supported()).await;
    let caller = caller(&setup);
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    let tools = delivery.tool_replies();
    let frame = recovery_frame("resp_1", 0, true);
    let (result, ()) = tokio::join!(
        tools.reply(caller, &channel, &event, message(), false),
        async {
            let reply = command(&mut rx).await;
            assert!(matches!(
                delivery
                    .deliver_frame(
                        &setup.plan.identity,
                        &setup.target().id,
                        &mut lease,
                        &frame,
                        ReplayEffect::NoEffect,
                    )
                    .await,
                Err(ExecutionError::Busy)
            ));
            assert_eq!(cursor(&lease.state), None);
            assert!(lease.state.delivery.as_ref().unwrap().status.settled());
            assert_eq!(
                setup.journal().complete(&mut lease, "resp_1").await,
                Err(ExecutionError::Invalid)
            );
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt()).unwrap();
        }
    );
    result.unwrap();
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &frame,
                ReplayEffect::NoEffect,
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { .. }
    ));
    setup
        .journal()
        .complete(&mut lease, "resp_1")
        .await
        .unwrap();
    let mut late = message();
    late.text = Some("a different late reply".into());
    assert!(matches!(
        tools
            .reply(caller, &channel, &event, late.clone(), false)
            .await,
        Err(ExecutionError::Invalid)
    ));
    // Explicit delayed targets retain their independent durable receipt path.
    assert!(tools
        .reply(caller, &channel, &event, late, true)
        .await
        .unwrap()
        .is_none());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn a_later_unknown_tool_blocks_terminal_even_after_an_earlier_confirmed_reply() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let store = ReplyStore(setup.journal().clone());
    let Admission::Claimed(first) = store
        .admit(&setup.plan, setup.target(), Kind::Tool, "a".repeat(64))
        .await
        .unwrap()
    else {
        panic!("claimed")
    };
    store.finish(&first, &receipt()).await.unwrap();
    let Admission::Claimed(second) = store
        .admit(&setup.plan, setup.target(), Kind::Tool, "b".repeat(64))
        .await
        .unwrap()
    else {
        panic!("claimed")
    };
    store
        .finish(
            &second,
            &receipt_for(DeliveryOutcome::Unknown {
                reason: "lost_response".into(),
            }),
        )
        .await
        .unwrap();
    let (delivery, mut rx) = lane(&setup);
    assert_eq!(
        delivery
            .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body,)
            .await
            .unwrap(),
        ToolReplyStatus::Unknown
    );
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &recovery_frame("resp_1", 0, true),
                ReplayEffect::NoEffect,
            )
            .await
            .unwrap(),
        FrameDelivery::Unknown
    ));
    assert_eq!(cursor(&lease.state), None);
    assert!(rx.try_recv().is_err());
}

mod terminal;
