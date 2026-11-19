use super::tests::fixture;
use crate::ingress_budget::{AllocationPlan, IngressClass};
use crate::{ConversationSettingsSupport, RuntimeOutputSource};

#[tokio::test]
async fn observers_outlive_the_bridge_without_releasing_watch_storage() {
    let fixture = fixture();
    fixture.activate().await;
    let root = fixture.root.clone();
    assert!(fixture.bridge.managed_context_progress().await.is_none());
    assert!(fixture.bridge.retained_history_deadline().await.is_none());
    let observation = fixture.bridge.delivery_observation().await;
    let progress = observation.admitted_progress().unwrap();
    let mut history = observation.admitted_history().unwrap();
    let held = progress.requested_bytes() + history.requested_bytes();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    fixture
        .bridge
        .retained_history_deadline
        .send_replace(Some(deadline));
    history.changed().await.unwrap();
    assert_eq!(history.take_latest(), Some(deadline));
    let copy = observation.clone();
    drop((fixture, observation));
    assert_eq!(root.capacity_bytes() - root.available_bytes(), held);
    assert!(history.changed().await.is_err());
    assert_eq!(progress.latest(), 0);
    drop((progress, history));
    assert_eq!(root.capacity_bytes() - root.available_bytes(), held);
    drop(copy);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn admitted_durable_watch_advances_after_ack_under_data_pressure() {
    let mut fixture = fixture();
    fixture.activate().await;
    let observation = fixture.bridge.delivery_observation().await;
    let mut progress = observation.admitted_progress().unwrap();
    let source = RuntimeOutputSource {
        stream_id: fixture.stream.to_string(),
        cursor: 7,
        native_conversation_id: "conversation".into(),
        payload_sha256: "0".repeat(64),
    };
    let state = fixture.bridge.output_ack_state().await.unwrap();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let pressure = fixture
        .policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("watch data pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    assert_eq!(progress.latest(), 0);
    fixture
        .bridge
        .acknowledge_output(&source, Some(&state))
        .await
        .unwrap();
    progress.changed().await.unwrap();
    assert_eq!(progress.take_latest(), 7);
    fixture
        .bridge
        .acknowledge_output(&source, Some(&state))
        .await
        .unwrap();
    assert!(
        !progress.has_changed().unwrap(),
        "duplicate ACK cannot extend delivery"
    );
    let earlier = RuntimeOutputSource {
        cursor: 6,
        ..source
    };
    fixture
        .bridge
        .acknowledge_output(&earlier, Some(&state))
        .await
        .unwrap();
    assert!(
        !progress.has_changed().unwrap(),
        "regressing ACK cannot extend delivery"
    );
    assert_eq!(fixture.writes.lock().unwrap().len(), 3);
    let authority = crate::owner_protocol::test_support::TestAuthority::new(1);
    fixture.bridge.authority = Some(authority.clone());
    crate::owner_protocol::TransportAuthority::revoke(&*authority);
    let refused = RuntimeOutputSource {
        cursor: 8,
        ..earlier
    };
    assert!(fixture
        .bridge
        .acknowledge_output(&refused, Some(&state))
        .await
        .is_err());
    assert!(!progress.has_changed().unwrap());
    assert_eq!(progress.latest(), 7);
    assert_eq!(fixture.writes.lock().unwrap().len(), 3);
    drop(pressure);
}
