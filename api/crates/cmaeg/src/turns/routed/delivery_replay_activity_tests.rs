use super::*;

#[tokio::test]
async fn exact_answer_replay_binds_the_retained_receipt_without_sending() {
    use crate::{
        activity::{facts, public_page, ActivityStore, Change, ConsumerState},
        ingress::{routing_plan::fixture, routing_store::test_server::Server},
        providers::ResponsesEvent,
        turns::observer::{DeliveryStep, Destination},
    };
    let setup = Setup::new(supported()).await;
    let server = Server::start_realtime().await;
    let activity = Arc::new(ActivityStore::new(
        server.records.clone(),
        Arc::new(fixture::keys()),
    ));
    let identity = &setup.plan.identity;
    let target = setup.target();
    let record = identity.record_id.as_str();
    let event = identity.event_id.as_str();
    activity
        .received_at(
            &identity.owner,
            record,
            &setup.plan.activity,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    let confirmation = DeliveryOutcome::Confirmed {
        platform_message_id: Some("message-1".into()),
    };
    let legacy = facts::delivery(
        target.launch.consumer.as_str(),
        DeliveryStep::Answer,
        &confirmation,
        Destination::Origin,
        &target.launch.event.reply,
        None,
    );
    let original_time = legacy
        .delivery
        .as_ref()
        .unwrap()
        .confirmed_at
        .clone()
        .unwrap();
    activity.change(record, event, &legacy).await.unwrap();

    let mut lease = accepted(&setup).await;
    let (mut delivery, mut rx) = lane(&setup);
    // The original production observation never reached the Activity store.
    let mut frame = recovery_frame("resp_1", 0, false);
    frame.event = ResponsesEvent::OutputTextDone {
        item_id: "item-1".into(),
        text: "private replay answer".into(),
    };
    frame.raw = serde_json::json!({"type":"response.output_text.done","response_id":"resp_1","sequence_number":0,"item_id":"item-1","text":"private replay answer"});
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(identity, &target.id, &mut lease, &frame, effect()),
        async {
            let reply = command(&mut rx).await;
            assert_eq!(reply.answer_response_id.as_deref(), Some("resp_1"));
            let _route = reply.gate.enter_route("fixture").await.unwrap();
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt_for(confirmation)).unwrap();
        }
    );
    assert!(matches!(result.unwrap(), FrameDelivery::Settled { .. }));
    let admitted = Change::state(
        target.launch.consumer.as_str(),
        ConsumerState::Accepted,
        Some("resp_1".into()),
    );
    activity.change(record, event, &admitted).await.unwrap();
    delivery.journal = delivery.journal.with_activity(activity.clone());
    let snapshot =
        || async { public_page(&activity.latest(&identity.owner, record).await.unwrap()) };
    let before = snapshot().await;
    let consumer = &before.items[0].consumers[0];
    assert!(consumer.produced_at.is_none());
    assert!(consumer.produced_by.is_none());
    assert!(consumer.deliveries[0].answer_response_id.is_none());

    let mut conflicting_frame = frame.clone();
    conflicting_frame.event = ResponsesEvent::OutputTextDone {
        item_id: "item-1".into(),
        text: "different answer".into(),
    };
    conflicting_frame.raw["text"] = "different answer".into();
    for (candidate, proposed_effect) in [
        (&conflicting_frame, effect()),
        (&frame, ReplayEffect::NoEffect),
    ] {
        assert!(matches!(
            delivery
                .deliver_frame(identity, &target.id, &mut lease, candidate, proposed_effect)
                .await,
            Err(ExecutionError::Invalid)
        ));
        assert_eq!(
            serde_json::to_value(snapshot().await).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        assert!(rx.try_recv().is_err());
    }

    assert!(matches!(
        delivery
            .deliver_frame(identity, &target.id, &mut lease, &frame, effect())
            .await
            .unwrap(),
        FrameDelivery::Settled { .. }
    ));
    assert!(rx.try_recv().is_err());
    let page = snapshot().await;
    let consumer = &page.items[0].consumers[0];
    assert!(consumer.produced_at.is_none());
    assert_eq!(
        consumer.produced_by.as_deref(),
        Some(original_time.as_str())
    );
    let receipts = &consumer.deliveries;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].answer_response_id.as_deref(), Some("resp_1"));
    assert_eq!(
        receipts[0].confirmed_at.as_deref(),
        Some(original_time.as_str())
    );
}
