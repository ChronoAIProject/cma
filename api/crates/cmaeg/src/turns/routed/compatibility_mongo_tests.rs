//! Independent authenticated clients retain an unrouted target through admission and completion.
use super::replica_test_fixture::*;
use crate::ingress::launch::LaunchRequest;
use cmaeg_core::seal::EventRef;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn compatibility_admission_replays_across_clients_without_reappending_completed_target() {
    let h = Harness::compatibility().await;
    assert!(h.plan.is_compatibility());
    assert!(h.plan.activity.routing.is_none());
    assert_eq!(h.plan.targets.len(), 1);
    let request = LaunchRequest {
        channel: h.current.channel.lock().unwrap().clone(),
        activity: h.plan.activity.clone(),
        conversation_id: h.target().launch.event.conversation_id.clone(),
        event_ref: EventRef::seal(&h.target().launch.event, &h.database.keys()).unwrap(),
        idempotency_key: h.target().launch.idempotency_key.clone(),
        consumer: None,
        received_at: std::time::Instant::now(),
        trace: tracing::Span::none(),
        timing: None,
    };
    let left = h.nodes[0].plans();
    let right = h.nodes[1].plans();
    let (a, b) = tokio::join!(
        left.replay_compatibility(&request, &h.plan.source_body, false),
        right.replay_compatibility(&request, &h.plan.source_body, false)
    );
    assert_eq!(a.unwrap(), Some(h.plan.clone()));
    assert_eq!(b.unwrap(), Some(h.plan.clone()));
    let (owner, mut lease) = h.accepted().await;
    h.finish_parent(owner, &mut lease).await;
    assert_eq!(
        right
            .replay_compatibility(&request, &h.plan.source_body, false)
            .await
            .unwrap(),
        Some(h.plan.clone())
    );
    assert!(
        h.nodes[1 - owner]
            .journal()
            .capture_partition(&h.plan, h.target())
            .await
            .is_err(),
        "completed work cannot obtain a new effect token"
    );
    h.cleanup().await;
}
