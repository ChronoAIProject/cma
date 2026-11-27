use super::{delivery_tests::*, test_support::*, *};
use crate::delivery::{durable::receipt_for, intents::DeliveryOutcome};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn recover_exact_automatic_receipt_when_the_frame_checkpoint_write_fails() {
    for outcome in [
        DeliveryOutcome::Confirmed {
            platform_message_id: Some("fixture-message".into()),
        },
        DeliveryOutcome::Rejected {
            reason: "fixture-refusal".into(),
        },
        DeliveryOutcome::Unknown {
            reason: "fixture-lost-receipt".into(),
        },
    ] {
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
                setup.server.fault(setup.target().id.clone(), false, true);
                reply.receipt.send(receipt_for(outcome.clone())).unwrap();
            }
        );
        assert!(result.is_err());
        let saved = stored(&setup).await;
        assert_eq!(cursor(&saved), None);
        assert!(matches!(
            saved.delivery.unwrap().status,
            DeliveryStatus::Intent
        ));
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);

        let mut resumed = accepted(&setup).await;
        assert!(matches!(
            delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    &mut resumed,
                    &frame,
                    ReplayEffect::NoEffect
                )
                .await,
            Err(ExecutionError::Invalid)
        ));
        let result = delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut resumed,
                &frame,
                effect(),
            )
            .await
            .unwrap();
        if matches!(outcome, DeliveryOutcome::Unknown { .. }) {
            assert!(matches!(result, FrameDelivery::Unknown));
            assert_eq!(cursor(&resumed.state), None);
        } else {
            let FrameDelivery::Settled {
                cursor: 0,
                receipt: Some(receipt),
            } = result
            else {
                panic!("expected original receipt");
            };
            assert_eq!(receipt.delivery, outcome);
            assert_eq!(cursor(&resumed.state), Some(0));
        }
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn receipt_persisted_before_lost_ack_reuses_receipt_after_restart() {
    for ack_stored in [false, true] {
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
                // The next write persists the receipt; only the cursor CAS fails.
                setup
                    .server
                    .fault_after(setup.target().id.clone(), ack_stored, true, 1);
                reply
                    .receipt
                    .send(receipt_for(DeliveryOutcome::Confirmed {
                        platform_message_id: Some("already-sent".into()),
                    }))
                    .unwrap();
            }
        );
        assert!(result.is_err());
        assert!(matches!(
            stored(&setup).await.delivery.unwrap().status,
            DeliveryStatus::Settled { .. }
        ));
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
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
            FrameDelivery::Settled { .. }
        ));
        assert_eq!(cursor(&resumed.state), Some(0));
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn a_new_lease_prevents_the_previous_queued_worker_from_sending() {
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
            setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
            let mut successor = accepted(&setup).await;
            let refusal = reply.gate.attempt().await.err().unwrap();
            assert!(!reply.gate.commit());
            assert!(matches!(
                delivery
                    .deliver_frame(
                        &setup.plan.identity,
                        &setup.target().id,
                        &mut successor,
                        &frame,
                        effect()
                    )
                    .await
                    .unwrap(),
                FrameDelivery::Unknown
            ));
            reply.receipt.send(receipt_for(refusal.outcome())).unwrap();
        }
    );
    assert!(matches!(result, Err(ExecutionError::LostLease)));
    assert_eq!(cursor(&stored(&setup).await), None);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn sibling_consumer_cannot_advance_or_reuse_another_consumers_receipt() {
    let setup = Setup::new(supported()).await;
    let mut first = accepted(&setup).await;
    acknowledge_no_effect(&setup, &mut first, "resp_1", 0, false).await;
    let sibling = &setup.plan.targets[1];
    let DispatchOutcome::Replay { mut lease, .. } = setup
        .dispatcher
        .dispatch(&setup.plan.identity, &sibling.id, "sibling-worker")
        .await
        .unwrap()
    else {
        panic!("accepted")
    };
    let (delivery, _) = lane(&setup);
    let frame = recovery_frame("resp_1", 0, false);
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &sibling.id,
                &mut first,
                &frame,
                ReplayEffect::NoEffect
            )
            .await,
        Err(ExecutionError::Invalid)
    ));
    assert_eq!(cursor(&lease.state), None);
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &sibling.id,
                &mut lease,
                &frame,
                ReplayEffect::NoEffect
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { .. }
    ));
    assert_eq!(cursor(&first.state), Some(0));
    assert_eq!(cursor(&lease.state), Some(0));
}
