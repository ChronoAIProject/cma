use super::{test_support::*, *};
use crate::delivery::{
    durable::{receipt_for, DeliveryCoordinator, DurableReply},
    intents::{DeliveryCommand, DeliveryOutcome},
    router::ShardedScheduler,
};
use cmaeg_core::message::OutboundMessage;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::mpsc;

pub(super) fn lane(setup: &Setup) -> (RoutedDelivery, mpsc::Receiver<DeliveryCommand>) {
    let (tx, rx) = mpsc::channel(1);
    (
        RoutedDelivery::new(
            setup.server.store(),
            setup.journal().clone(),
            setup.current.clone(),
            DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))),
        ),
        rx,
    )
}
pub(super) fn effect() -> ReplayEffect {
    ReplayEffect::Reply(Box::new(OutboundMessage {
        text: Some("private replay answer".into()),
        blocks: vec![],
        reply_to: None,
        structured_content: None,
        reply_with_received_sticker: false,
        clear_native_attachments: false,
        native_media_update: None,
        native_attachments: vec![],
        whatsapp_template: None,
        thread_reply: false,
    }))
}
pub(super) async fn accepted(setup: &Setup) -> Box<Lease> {
    let DispatchOutcome::Replay { lease, .. } = setup.dispatch().await.unwrap() else {
        panic!("accepted")
    };
    lease
}
pub(super) async fn command(rx: &mut mpsc::Receiver<DeliveryCommand>) -> Box<DurableReply> {
    let DeliveryCommand::DurableReply(reply) = rx.recv().await.unwrap() else {
        panic!("durable")
    };
    reply
}
pub(super) async fn stored(setup: &Setup) -> TargetState {
    setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap()
        .0
}
pub(super) fn cursor(state: &TargetState) -> Option<u64> {
    let Phase::Accepted { durable_cursor, .. } = state.phase else {
        panic!("accepted")
    };
    durable_cursor
}

#[tokio::test]
async fn actual_receipt_is_durable_before_ack_and_exact_replay_never_resends() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let (delivery, mut rx) = lane(&setup);
    let frame = recovery_frame("resp_1", 0, false);
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            effect()
        ),
        async {
            let reply = command(&mut rx).await;
            let state = stored(&setup).await;
            assert!(matches!(
                state.delivery.unwrap().status,
                DeliveryStatus::Intent
            ));
            assert_eq!(
                cursor(&stored(&setup).await),
                None,
                "enqueue is not receipt"
            );
            assert_eq!(
                reply.event.consumer.as_ref(),
                Some(&setup.target().launch.consumer)
            );
            let _route = reply.gate.enter_route("fixture").await.unwrap();
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Confirmed {
                    platform_message_id: Some("message-1".into()),
                }))
                .unwrap();
        }
    );
    assert!(matches!(
        result.unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));
    assert_eq!(cursor(&lease.state), Some(0));
    assert!(matches!(&lease.state.delivery.as_ref().unwrap().status,
        DeliveryStatus::Settled { receipt: Some(receipt) } if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. })));
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
        FrameDelivery::Settled { .. }
    ));
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &frame,
                ReplayEffect::NoEffect
            )
            .await,
        Err(ExecutionError::Invalid)
    ));
    assert_eq!(
        setup.journal().complete(&mut lease, "resp_1").await,
        Err(ExecutionError::Invalid)
    );
}

#[tokio::test]
async fn unknown_and_lost_worker_receipt_block_replay_and_later_cursors() {
    for lost in [false, true] {
        let setup = Setup::new(supported()).await;
        let mut lease = accepted(&setup).await;
        let (delivery, mut rx) = lane(&setup);
        let frame = recovery_frame("resp_1", 0, false);
        let (result, ()) = tokio::join!(
            delivery.deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &frame,
                effect()
            ),
            async {
                let reply = command(&mut rx).await;
                if !lost {
                    reply
                        .receipt
                        .send(receipt_for(DeliveryOutcome::Unknown {
                            reason: "response_lost".into(),
                        }))
                        .unwrap();
                }
            }
        );
        assert!(matches!(result.unwrap(), FrameDelivery::Unknown));
        assert_eq!(cursor(&lease.state), None);
        setup.journal().release(&mut lease).await.unwrap();
        let mut resumed = accepted(&setup).await;
        assert!(matches!(
            delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    &mut resumed,
                    &frame,
                    effect()
                )
                .await
                .unwrap(),
            FrameDelivery::Unknown
        ));
        assert!(matches!(
            delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    &mut resumed,
                    &recovery_frame("resp_1", 1, false),
                    ReplayEffect::NoEffect
                )
                .await
                .unwrap(),
            FrameDelivery::Unknown
        ));
        assert!(rx.try_recv().is_err());
        assert_eq!(
            setup.journal().complete(&mut resumed, "resp_1").await,
            Err(ExecutionError::Invalid)
        );
    }
}

#[tokio::test]
async fn abandoned_intent_fences_queued_send_and_is_unknown_after_restart() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let (delivery, mut rx) = lane(&setup);
    let frame = recovery_frame("resp_1", 0, false);
    let mut pending = Box::pin(delivery.deliver_frame(
        &setup.plan.identity,
        &setup.target().id,
        &mut lease,
        &frame,
        effect(),
    ));
    let reply = tokio::select! { reply = command(&mut rx) => reply, _ = &mut pending => panic!("queued only") };
    drop(pending);
    assert!(!reply.gate.commit());
    assert!(reply.gate.attempt().await.is_err());
    setup.journal().release(&mut lease).await.unwrap();
    let mut resumed = accepted(&setup).await;
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut resumed,
                &frame,
                effect()
            )
            .await
            .unwrap(),
        FrameDelivery::Unknown
    ));
    assert!(matches!(
        stored(&setup).await.delivery.unwrap().status,
        DeliveryStatus::Unknown
    ));
    assert_eq!(cursor(&resumed.state), None);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn queue_refusal_is_a_durable_proof_of_no_send() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let (delivery, rx) = lane(&setup);
    drop(rx);
    let result = delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &recovery_frame("resp_1", 0, false),
            effect(),
        )
        .await
        .unwrap();
    assert!(
        matches!(result, FrameDelivery::Settled { receipt: Some(receipt), .. }
        if receipt.delivery == DeliveryOutcome::Rejected { reason: "delivery_closed".into() })
    );
    assert_eq!(cursor(&stored(&setup).await), Some(0));
}

#[tokio::test]
async fn no_effect_requires_receipt_and_contiguous_frames_and_terminal_proof() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    assert_eq!(
        setup
            .journal()
            .acknowledge_delivery(&mut lease, "resp_1", 0)
            .await,
        Err(ExecutionError::Invalid)
    );
    let (delivery, mut rx) = lane(&setup);
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &recovery_frame("resp_1", 1, false),
                ReplayEffect::NoEffect
            )
            .await,
        Err(ExecutionError::Invalid)
    ));
    acknowledge_no_effect(&setup, &mut lease, "resp_1", 0, false).await;
    assert_eq!(cursor(&stored(&setup).await), Some(0));
    acknowledge_no_effect(&setup, &mut lease, "resp_1", 1, true).await;
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &recovery_frame("resp_1", 2, false),
                ReplayEffect::NoEffect
            )
            .await,
        Err(ExecutionError::Invalid)
    ));
    setup
        .journal()
        .complete(&mut lease, "resp_1")
        .await
        .unwrap();
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn authority_revoked_after_enqueue_prevents_network_attempt() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let (delivery, mut rx) = lane(&setup);
    let frame = recovery_frame("resp_1", 0, false);
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            effect()
        ),
        async {
            let reply = command(&mut rx).await;
            setup.current.revoke_at.store(0, Ordering::SeqCst);
            let refusal = reply.gate.attempt().await.err().unwrap();
            reply.receipt.send(receipt_for(refusal.outcome())).unwrap();
        }
    );
    assert!(
        matches!(result.unwrap(), FrameDelivery::Settled { receipt: Some(receipt), .. }
        if matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }))
    );
}

#[tokio::test]
async fn changed_frame_or_foreign_response_is_rejected_without_advancing() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    acknowledge_no_effect(&setup, &mut lease, "resp_1", 0, false).await;
    let (delivery, mut rx) = lane(&setup);
    for mut frame in [
        recovery_frame("resp_1", 0, false),
        recovery_frame("other", 1, false),
    ] {
        frame.raw["untrusted"] = serde_json::json!(true);
        assert!(matches!(
            delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    &mut lease,
                    &frame,
                    ReplayEffect::NoEffect
                )
                .await,
            Err(ExecutionError::Invalid)
        ));
    }
    assert_eq!(cursor(&lease.state), Some(0));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn ambiguous_intent_write_without_readback_never_enqueues() {
    for stored in [false, true] {
        let setup = Setup::new(supported()).await;
        let mut lease = accepted(&setup).await;
        let (delivery, mut rx) = lane(&setup);
        setup.server.fault(setup.target().id.clone(), stored, true);
        assert!(delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &recovery_frame("resp_1", 0, false),
                effect()
            )
            .await
            .is_err());
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn exact_cas_readback_resolves_lost_write_and_expiry_never_extends_authority() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    setup.server.fault(setup.target().id.clone(), true, false);
    acknowledge_no_effect(&setup, &mut lease, "resp_1", 0, false).await;
    assert_eq!(cursor(&lease.state), Some(0));
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 901, Ordering::SeqCst);
    let mut lease = accepted(&setup).await;
    let (delivery, mut rx) = lane(&setup);
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &recovery_frame("resp_1", 1, false),
                effect()
            )
            .await,
        Err(ExecutionError::Expired)
    ));
    assert!(rx.try_recv().is_err());
}
