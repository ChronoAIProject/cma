use super::*;
use native_pool::NativeHttpLane::{Control, Ownership};

#[tokio::test]
async fn fresh_lane_admission_fences_old_siblings_and_refuses_late_old_endpoints() {
    for (requested, sibling) in [(Control, Ownership), (Ownership, Control)] {
        for cached_requested in [false, true] {
            for restart in [false, true] {
                let fixture = fixture().await;
                let old_endpoint = fixture.endpoint().await;
                let old_sibling = fixture
                    .transport
                    .native_slot_for(&old_endpoint, sibling)
                    .await
                    .unwrap();
                let old_requested = if cached_requested {
                    Some(
                        fixture
                            .transport
                            .native_slot_for(&old_endpoint, requested)
                            .await
                            .unwrap(),
                    )
                } else {
                    None
                };
                if restart {
                    fixture.state.restarted.store(true, Ordering::SeqCst);
                } else {
                    fixture.state.replaced.store(true, Ordering::SeqCst);
                }
                let RuntimeDiscovery::AgentSandbox(client) = &fixture.transport.client else {
                    unreachable!()
                };
                let binding = client.pod_binding(&handle()).await.unwrap();
                let fresh_endpoint = old_endpoint.clone().with_native_binding(handle(), binding);
                let new_requested = fixture
                    .transport
                    .native_slot_for(&fresh_endpoint, requested)
                    .await
                    .unwrap();
                assert!(*old_sibling.cancellation().borrow());
                if let Some(old) = old_requested {
                    assert!(*old.cancellation().borrow());
                }
                let new_sibling = fixture
                    .transport
                    .native_slot_for(&fresh_endpoint, sibling)
                    .await
                    .unwrap();
                // A delayed P1 endpoint must not evict either admitted P2 lane.
                assert!(fixture
                    .transport
                    .native_slot_for(&old_endpoint, requested)
                    .await
                    .is_err());
                assert!(!*new_requested.cancellation().borrow());
                assert!(!*new_sibling.cancellation().borrow());
                assert_eq!(fixture.transport.state.lock().await.native_http.len(), 2);
                assert!(fixture.state.requests.lock().unwrap().is_empty());
                fixture.transport.retire(&handle()).await.unwrap();
                fixture.drained().await;
            }
        }
    }
}
