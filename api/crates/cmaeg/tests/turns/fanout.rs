use super::{
    fanout_support::*,
    routing_fixture::{NYX, REVIEWER},
};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::DeliveryCommand,
};
use cmaeg_core::{
    ids::{ConversationId, IdempotencyKey},
    seal::{EventRef, ReplyAddress},
};
use std::{sync::Arc, time::Instant};
use tokio::sync::Semaphore;

#[tokio::test]
async fn children_keep_one_activity_but_independent_identity_and_primary_sync() {
    for sync_fanout in [false, true] {
        let h = Harness::new().await;
        let mut request = h.request(&["default", REVIEWER, NYX]);
        request.sync_fanout = sync_fanout;
        request.timing = Some(cmaeg::telemetry::timing::IngressTiming::new("custom"));
        let original_conversation = request.conversation_id.clone();
        let original_key = request.idempotency_key.clone();
        let activity = request.activity.clone();
        let mut base = request
            .event_ref
            .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
            .unwrap()
            .payload;
        base.reply = ReplyAddress::Sync {
            waiter: "fixture-waiter".into(),
        };
        request.event_ref = EventRef::seal(&base, &keys()).unwrap();
        let permits = Arc::new(Semaphore::new(1));
        let result = h
            .launcher()
            .launch_routed(
                request,
                Some(permits.clone().acquire_owned().await.unwrap()),
            )
            .await;
        assert!(matches!(result.combined, LaunchOutcome::Accepted { .. }));
        assert_eq!(result.primary_conversation, original_conversation);
        let children = h.recorder.requests.lock().unwrap();
        assert_eq!(children.len(), 3);
        for (i, (child, held)) in children.iter().enumerate() {
            assert_eq!(child.activity, activity);
            assert_eq!(*held, i == 0);
            assert_eq!(child.timing.is_some(), i == 0);
            let payload = child
                .event_ref
                .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
                .unwrap()
                .payload;
            assert!(payload.verifies_activity(&activity));
            assert_eq!(payload.consumer, child.consumer);
            assert_eq!(payload.conversation_id, child.conversation_id);
            assert_eq!(
                matches!(payload.reply, ReplyAddress::Sync { .. }),
                i == 0 || sync_fanout
            );
            if i == 0 {
                assert_eq!(child.conversation_id, original_conversation);
                assert_eq!(child.idempotency_key, original_key);
                assert!(child.consumer.is_none());
            } else {
                let id = child.consumer.as_ref().unwrap();
                assert_eq!(
                    child.conversation_id,
                    ConversationId::derive_for_consumer(
                        &keys(),
                        &child.channel.id,
                        child.channel.definition.partition.version,
                        id,
                        &base.partition_key
                    )
                );
                assert_eq!(
                    child.idempotency_key,
                    IdempotencyKey::for_consumer(&activity.event_id, id)
                );
                assert_ne!(child.event_ref, children[i - 1].0.event_ref);
                assert_ne!(child.conversation_id, children[i - 1].0.conversation_id);
            }
        }
        assert_eq!(permits.available_permits(), 1);
        assert_eq!(h.observer.0.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn primary_names_first_admitted_even_when_a_later_child_is_accepted() {
    let mut h = Harness::new().await;
    h.recorder
        .outcomes
        .lock()
        .unwrap()
        .insert("default".into(), refused("provider_busy"));
    h.recorder
        .outcomes
        .lock()
        .unwrap()
        .insert(REVIEWER.into(), LaunchOutcome::Deferred);
    let result = h
        .launcher()
        .launch_routed(h.request(&["default", REVIEWER, NYX]), None)
        .await;
    assert!(
        matches!(result.combined, LaunchOutcome::Accepted { response_id } if response_id == format!("response-{NYX}"))
    );
    assert_eq!(
        result.primary_conversation,
        result.consumers[1].conversation_id
    );
    assert!(
        matches!(h.failure().await, DeliveryCommand::ConsumerFailed { consumer, code: "provider_busy", .. } if consumer.is_default())
    );
    idle(&h.base.tracker).await;
    assert!(h.outcomes.try_recv().is_err());
}

#[tokio::test]
async fn removed_pending_and_capacity_refusals_preserve_child_identity() {
    let mut h = Harness::new().await;
    let mut request = h.request(&["default", REVIEWER, NYX]);
    request.channel = super::preparation_support::rebound(&request.channel, |def| {
        let routing = def.routing.as_mut().unwrap();
        routing.consumers.retain(|c| c.id.as_str() != REVIEWER);
        routing.consumers[0].pending = true;
    });
    let result = h.launcher().launch_routed(request, None).await;
    assert_eq!(h.recorder.requests.lock().unwrap().len(), 1);
    for (i, target) in [REVIEWER, NYX].iter().enumerate() {
        let DeliveryCommand::ConsumerFailed {
            consumer,
            payload,
            code,
            ..
        } = h.failure().await
        else {
            panic!()
        };
        assert_eq!(consumer.as_str(), *target);
        assert_eq!(payload.consumer.as_ref(), Some(&consumer));
        assert_eq!(
            payload.conversation_id,
            result.consumers[i + 1].conversation_id
        );
        assert_eq!(code, "consumer_removed");
    }
    let leases = (0..3)
        .map(|_| {
            h.capacity
                .try_lease(
                    &h.base.channel.pointer.record_id,
                    &h.base.channel.definition.owner.subject,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    let result = h
        .launcher()
        .launch_routed(h.request(&["default", REVIEWER]), None)
        .await;
    elapsed_bound(started);
    assert!(
        matches!(&result.consumers[1].outcome, LaunchOutcome::Refused(r) if r.code == "channel_capacity")
    );
    assert!(matches!(
        h.failure().await,
        DeliveryCommand::ConsumerFailed {
            code: "channel_capacity",
            ..
        }
    ));
    drop(leases);
}

#[tokio::test]
async fn all_refused_returns_primary_problem_without_delivery_and_releases_hold() {
    let mut h = Harness::new().await;
    let request = h.request(&["default", REVIEWER]);
    let leases = (0..4)
        .map(|_| {
            h.capacity
                .try_lease(
                    &request.channel.pointer.record_id,
                    &request.channel.definition.owner.subject,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let permits = Arc::new(Semaphore::new(1));
    let result = h
        .launcher()
        .launch_routed(
            request,
            Some(permits.clone().acquire_owned().await.unwrap()),
        )
        .await;
    assert!(
        matches!(result.combined, LaunchOutcome::Refused(r) if r.code == "channel_capacity" && r.problem.status == 503)
    );
    assert!(h.recorder.requests.lock().unwrap().is_empty());
    assert_eq!(permits.available_permits(), 1);
    idle(&h.base.tracker).await;
    assert!(h.outcomes.try_recv().is_err());
    drop(leases);
}

#[tokio::test]
async fn single_target_and_legacy_delegation_register_no_finalizer() {
    let mut h = Harness::new().await;
    let result = h
        .launcher()
        .launch_routed(h.request(&[REVIEWER]), None)
        .await;
    assert!(matches!(result.combined, LaunchOutcome::Accepted { .. }));
    assert!(h.base.tracker.is_empty());
    assert!(h.outcomes.try_recv().is_err());
    let mut request = h.request(&[REVIEWER]);
    request.activity.routing = None;
    request.consumer = Some(cmaeg_core::ids::ConsumerId::parse(REVIEWER).unwrap());
    let conversation = request.conversation_id.clone();
    let result = h.launcher().launch_routed(request, None).await;
    assert_eq!(result.consumers[0].id.as_str(), REVIEWER);
    assert_eq!(result.primary_conversation, conversation);
    assert_eq!(h.observer.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn refused_primary_never_transfers_sync_timing_or_ingress_permit_to_a_sibling() {
    let mut h = Harness::new().await;
    let mut request = h.request(&[REVIEWER, "default"]);
    request.timing = Some(cmaeg::telemetry::timing::IngressTiming::new("custom"));
    request.channel = super::preparation_support::rebound(&request.channel, |d| {
        d.routing.as_mut().unwrap().consumers.clear()
    });
    let mut payload = request
        .event_ref
        .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
        .unwrap()
        .payload;
    payload.reply = ReplyAddress::Sync {
        waiter: "fixture-waiter".into(),
    };
    request.event_ref = EventRef::seal(&payload, &keys()).unwrap();
    let permits = Arc::new(Semaphore::new(1));
    let result = h
        .launcher()
        .launch_routed(
            request,
            Some(permits.clone().acquire_owned().await.unwrap()),
        )
        .await;
    assert_eq!(
        result.primary_conversation,
        result.consumers[1].conversation_id
    );
    assert_eq!(permits.available_permits(), 1);
    {
        let children = h.recorder.requests.lock().unwrap();
        assert_eq!(children.len(), 1);
        assert!(!children[0].1);
        assert!(children[0].0.timing.is_none());
        let payload = children[0]
            .0
            .event_ref
            .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
            .unwrap()
            .payload;
        assert_eq!(payload.reply, ReplyAddress::None);
    }
    let DeliveryCommand::ConsumerFailed { payload, .. } = h.failure().await else {
        panic!()
    };
    assert!(matches!(payload.reply, ReplyAddress::Sync { .. }));
}

#[tokio::test]
async fn observer_measures_each_acceptance_without_sibling_delay() {
    let h = Harness::new().await;
    h.recorder
        .delays
        .lock()
        .unwrap()
        .insert(REVIEWER.into(), std::time::Duration::from_millis(100));
    h.launcher()
        .launch_routed(h.request(&["default", REVIEWER]), None)
        .await;
    let observed = h.observer.0.lock().unwrap();
    let fast = observed
        .iter()
        .find(|o| o.consumer.is_default())
        .unwrap()
        .accepted_ms
        .unwrap();
    let slow = observed
        .iter()
        .find(|o| o.consumer.as_str() == REVIEWER)
        .unwrap()
        .accepted_ms
        .unwrap();
    assert!(slow >= fast + 70, "fast={fast} slow={slow}");
}
