use super::{test_server::Server, Candidate};
use crate::ingress::{message_event, routing_plan::fixture};
use cmaeg_core::activity::{Activity, ActorKind};
use serde_json::json;

fn notification(original: &Activity) -> Activity {
    let mut event = original.clone();
    event.event_id = "message-change-1".parse().unwrap();
    event.actor.id = "unknown".into();
    event.actor.kind = ActorKind::Unknown;
    event.kind = serde_json::from_value(json!({"type":"event","ce":{
        "specversion":"1.0","id":"message-change-1","source":"urn:cmaeg:telegram:77",
        "type":"cma.message.deleted.v1","subject":"1",
        "data":{"message_id":"1","original_event_id":original.event_id}
    }}))
    .unwrap();
    event
}

#[tokio::test]
async fn retained_message_events_keep_all_original_targets_partitions_and_conversations() {
    let server = Server::start().await;
    let original = fixture::request(fixture::targets, fixture::message("original"));
    let candidate = Candidate::prepare(
        &original,
        &fixture::routed(&original),
        b"original",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let original = server.store().admit(&candidate).await.unwrap();
    let incoming = fixture::request(
        |_| {},
        notification(&{
            let mut a = original.activity.clone();
            a.routing = None;
            a
        }),
    );
    let (request, plan) =
        message_event::bind(incoming, &original, &fixture::keys(), fixture::NOW).unwrap();
    assert_eq!(plan.targets.len(), 3);
    for (before, after) in original.targets.iter().zip(&plan.targets) {
        assert_eq!(before.launch.consumer, after.consumer);
        assert_eq!(before.launch.provider, after.provider);
        assert_eq!(
            before.launch.event.conversation_id,
            after.event.conversation_id
        );
        assert_eq!(before.launch.event.partition_key, after.event.partition_key);
        assert_ne!(before.launch.idempotency_key, after.idempotency_key);
        assert!(after.event.verifies_activity(&request.activity));
    }
    assert_eq!(request.activity.actor.kind, ActorKind::Unknown);
    let upload = Candidate::prepare_message_event(
        &request,
        &plan,
        b"change",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let admitted = server.store().admit(&upload).await.unwrap();
    assert_eq!(
        server
            .store()
            .replay_message_event(&request, b"change")
            .await
            .unwrap(),
        Some(admitted.clone())
    );
    assert_eq!(
        server.store().load(&admitted.identity).await.unwrap(),
        Some(admitted)
    );
    assert!(server.store().replay(&request, b"change").await.is_err());
}

#[tokio::test]
async fn message_events_cannot_retarget_another_owner_source_channel_or_provider() {
    let server = Server::start().await;
    let original_request = fixture::request(|_| {}, fixture::message("original"));
    let original = server
        .store()
        .admit(
            &Candidate::prepare(
                &original_request,
                &fixture::routed(&original_request),
                b"original",
                &fixture::keys(),
                fixture::NOW,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    for defect in 0..7 {
        let mut activity = notification(&original_request.activity);
        if defect == 0 {
            activity.conversation.id = "43".into();
        }
        if defect == 1 {
            activity.source = cmaeg_core::activity::SourceKind::Custom;
        }
        if defect == 2 {
            let cmaeg_core::activity::ActivityKind::Event { ce, .. } = &mut activity.kind else {
                panic!()
            };
            ce.data.as_mut().unwrap()["original_event_id"] = "other-event".into();
        }
        let request = fixture::request(
            |value| {
                if defect == 3 {
                    value["owner"]["subject"] = "other-owner".into();
                }
            },
            activity,
        );
        let mut old = original.clone();
        if defect == 4 {
            old.targets[0].launch.provider.slug = "changed-provider".parse().unwrap();
        }
        if defect == 5 {
            old.identity.record_id = "22222222-2222-4222-8222-222222222222".parse().unwrap();
        }
        if defect == 6 {
            old.retained_until = fixture::NOW;
        }
        assert!(
            message_event::bind(request, &old, &fixture::keys(), fixture::NOW).is_err(),
            "defect {defect}"
        );
    }
}
