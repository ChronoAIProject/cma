use super::super::tests::{settings, socket_pair, stop};
use super::budget_tests::{open_with_coordinator, retained};
use super::*;
use futures_util::StreamExt as _;
use std::time::Duration;

#[tokio::test]
async fn process_pressure_refuses_before_cursor_but_durable_ack_still_progresses() {
    let root = IngressBudget::process();
    let before = root.available_bytes();
    let (opened, mut server, _received) = open_with_coordinator(true, true, true, true).await;
    let baseline = root.available_bytes();
    assert!(baseline < before, "permission table is charged before use");
    assert!(opened
        .bridge
        .output
        .as_ref()
        .unwrap()
        .lock()
        .await
        .aggregate
        .is_some());
    let other_component = Arc::new(Semaphore::new(before));
    let blocker = IngressReservation::try_acquire(&other_component, Some(&root), baseline).unwrap();
    let (capture, mut effects) = UpdateRelay::capture();
    let frame = retained(3, b"{}");
    assert!(matches!(
        opened.bridge.receive_output(&frame, &capture).await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert!(effects.try_recv().is_err());
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .last_received,
        Some(2)
    );
    assert_eq!(root.available_bytes(), 0);
    drop(blocker);
    opened
        .bridge
        .receive_output(&frame, &capture)
        .await
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, .. } = effects.try_recv().unwrap() else {
        panic!("accepted frame requires its original durable receipt")
    };
    assert_eq!(receipt.source.cursor, 3);
    assert_eq!(root.available_bytes(), baseline);
    let blocker = IngressReservation::try_acquire(&other_component, Some(&root), baseline).unwrap();
    receipt.acknowledge().await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Binary(bytes) = ack else {
        panic!("durable ACK must progress without data capacity")
    };
    assert_eq!(bytes[0], LANE_OUTPUT_ACK);
    assert_eq!(u64::from_be_bytes(bytes[17..25].try_into().unwrap()), 3);
    assert_eq!(root.available_bytes(), 0);
    drop(blocker);
    let lifetime = Arc::downgrade(&opened.bridge);
    stop(opened).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while lifetime.strong_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(root.available_bytes(), before);
}

#[tokio::test]
async fn invalid_coordinator_policy_refuses_before_reading_ready() {
    for missing in 0..3 {
        let (client, _server) = socket_pair().await;
        let mut settings = settings(None, false);
        settings.coordinated_ingress = true;
        settings.retained_json = missing != 0;
        settings.bounded_raw_output = missing != 1;
        settings.bounded_permission_state = missing != 2;
        let (sender, _) = mpsc::channel(1);
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            Bridge::open(
                client,
                "agent-1",
                "/workspace".into(),
                settings,
                None,
                sender,
            ),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Err(AdapterError::Rejected(reason)) if reason.contains("ingress coordination requires"))
        );
    }
}

#[tokio::test]
async fn disabled_coordinator_leaves_independent_component_policies_unchanged() {
    let root = IngressBudget::process();
    let before = root.available_bytes();
    let (opened, _server, _received) = open_with_coordinator(true, true, true, false).await;
    assert!(opened
        .bridge
        .output
        .as_ref()
        .unwrap()
        .lock()
        .await
        .aggregate
        .is_none());
    assert_eq!(root.available_bytes(), before);
    stop(opened).await;
}
