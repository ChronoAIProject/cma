use super::{fanout_support::*, routing_fixture::REVIEWER};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::DeliveryCommand,
};
use cmaeg_core::{
    ids::{ConsumerId, ConversationId, RecordId},
    seal::{EventRef, EventRefPayload},
};

#[tokio::test]
async fn only_the_oversized_child_is_refused_and_its_lease_is_released() {
    let mut h = Harness::new().await;
    let mut request = h.request(&["default", REVIEWER]);
    let mut base = request
        .event_ref
        .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
        .unwrap()
        .payload;
    // Choose the last valid base size for which adding a non-default consumer
    // exceeds the real token limit. This uses the actual serializer/sealer.
    let mut selected = None;
    for n in (1..=4096).rev() {
        base.partition_key = "p".repeat(n);
        let Ok(sealed) = EventRef::seal(&base, &keys()) else {
            continue;
        };
        let mut child = base.clone();
        child.consumer = Some(ConsumerId::parse(REVIEWER).unwrap());
        child.conversation_id = ConversationId::derive_for_consumer(
            &keys(),
            &request.channel.id,
            request.channel.definition.partition.version,
            child.consumer.as_ref().unwrap(),
            &child.partition_key,
        );
        if EventRef::seal(&child, &keys()).is_err() {
            selected = Some(sealed);
            break;
        }
    }
    request.event_ref = selected.expect("a base payload just below the seal limit");
    let result = h.launcher().launch_routed(request, None).await;
    assert!(matches!(result.combined, LaunchOutcome::Accepted { .. }));
    assert!(
        matches!(&result.consumers[1].outcome, LaunchOutcome::Refused(r) if r.code == "event_ref_too_large" && r.problem.status == 413)
    );
    assert_eq!(h.recorder.requests.lock().unwrap().len(), 1);
    assert!(matches!(
        h.failure().await,
        DeliveryCommand::ConsumerFailed {
            code: "event_ref_too_large",
            ..
        }
    ));
    let leases = (0..4)
        .map(|_| {
            h.capacity
                .try_lease(
                    &h.base.channel.pointer.record_id,
                    &h.base.channel.definition.owner.subject,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    drop(leases);
}

#[tokio::test]
async fn seventeenth_owner_turn_is_refused_without_consuming_channel_capacity() {
    let mut h = Harness::new().await;
    let mut leases = Vec::new();
    for i in 0..4 {
        let record: RecordId = format!("00000000-0000-4000-8000-{i:012x}").parse().unwrap();
        for _ in 0..4 {
            leases.push(
                h.capacity
                    .try_lease(&record, &h.base.channel.definition.owner.subject)
                    .unwrap(),
            );
        }
    }
    let result = h
        .launcher()
        .launch_routed(h.request(&[REVIEWER]), None)
        .await;
    assert!(matches!(result.combined, LaunchOutcome::Refused(r) if r.code == "owner_capacity"));
    assert!(h.outcomes.try_recv().is_err());
    assert!(h.recorder.requests.lock().unwrap().is_empty());
    drop(
        h.capacity
            .try_lease(&h.base.channel.pointer.record_id, "other-owner")
            .unwrap(),
    );
    drop(leases);
}

#[tokio::test]
async fn invalid_base_and_invalid_targets_never_launch_or_hold_capacity() {
    let h = Harness::new().await;
    for case in 0..4 {
        let mut request = h.request(&["default", REVIEWER]);
        let expected = match case {
            0 => {
                request.event_ref = EventRef::from_sealed("cmaeg1.ev.invalid".into());
                "event_ref_invalid"
            }
            1 => {
                let mut payload: EventRefPayload = request
                    .event_ref
                    .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
                    .unwrap()
                    .payload;
                payload.expires_at = 1;
                request.event_ref = EventRef::seal(&payload, &keys()).unwrap();
                "event_ref_expired"
            }
            2 => {
                request.activity.routing.as_mut().unwrap().targets.clear();
                "invalid_targets"
            }
            _ => {
                let routing = request.activity.routing.as_mut().unwrap();
                routing.targets = vec![routing.targets[0].clone(); 9];
                "invalid_targets"
            }
        };
        let result = h.launcher().launch_routed(request, None).await;
        assert!(matches!(result.combined, LaunchOutcome::Refused(r) if r.code == expected));
        assert!(h.recorder.requests.lock().unwrap().is_empty());
        assert!(h.base.tracker.is_empty());
    }
}
