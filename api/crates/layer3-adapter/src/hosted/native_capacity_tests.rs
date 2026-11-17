use super::*;
use crate::agent_sandbox::{RuntimeCapacityReason, RuntimePermitKind, RuntimeWorkloadClass};
use crate::hosted::HostedControlTransport;

#[tokio::test]
async fn last_request_permit_opens_a_new_connection_once_and_idle_reuse_holds_none() {
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        let fixture = Fixture::start(Mode::Persistent, mode).await;
        fixture
            .transport
            .bind_native_runtime(&handle(), "sandbox-capacity", &"a".repeat(64))
            .await
            .unwrap();
        let endpoint = fixture.endpoint().await;
        let authority = &fixture.transport.runtime_capacity;
        let held: Vec<_> = (0..31)
            .map(|_| {
                authority
                    .try_acquire(RuntimePermitKind::Request, RuntimeWorkloadClass::Control)
                    .unwrap()
            })
            .collect();
        assert_eq!(authority.available_requests(), 1);
        for expected_opens in [1, 1] {
            tokio::time::timeout(
                Duration::from_secs(2),
                fixture
                    .transport
                    .control(&endpoint, &RequestFrame::Resources),
            )
            .await
            .expect("no nested request permit wait")
            .unwrap();
            assert_eq!(
                authority.available_requests(),
                1,
                "cached idle socket must not pin a request permit"
            );
            assert_eq!(fixture.state.opened.load(Ordering::SeqCst), expected_opens);
        }
        let last = authority
            .try_acquire(RuntimePermitKind::Request, RuntimeWorkloadClass::Control)
            .unwrap();
        let result = fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await;
        assert!(
            matches!(result, Err(AdapterError::RuntimeCapacity(ref failure)) if failure.reason == RuntimeCapacityReason::Exhausted)
        );
        drop((last, held));
        assert_eq!(authority.available_requests(), 32);
        fixture.transport.retire(&handle()).await.unwrap();
    }
}

#[tokio::test]
async fn sixteen_busy_http_slots_refuse_by_name_and_recover_after_release() {
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    let RuntimeDiscovery::AgentSandbox(client) = &fixture.transport.client else {
        unreachable!()
    };
    let mut held = Vec::new();
    for index in 0..17 {
        let name = format!("slot-{index}");
        let handle = RuntimeHandle::new(format!("agent-sandbox:{name}:{}", sandbox_uid(&name)));
        fixture
            .state
            .runtimes
            .lock()
            .unwrap()
            .insert(name.clone(), client.control_token(&name));
        fixture
            .transport
            .bind_native_runtime(&handle, &format!("sandbox-{index}"), &"a".repeat(64))
            .await
            .unwrap();
        let endpoint = fixture.transport.endpoint(&handle).await.unwrap();
        if index < 16 {
            held.push(fixture.transport.native_slot(&endpoint).await.unwrap());
        } else {
            let error = fixture
                .transport
                .native_slot(&endpoint)
                .await
                .err()
                .unwrap();
            assert!(
                matches!(error, AdapterError::RuntimeCapacity(ref failure) if failure.reason == RuntimeCapacityReason::NativeHttpSessions && failure.code() == "runtime_request_capacity_exhausted")
            );
            drop(held.pop());
            assert!(fixture.transport.native_slot(&endpoint).await.is_ok());
        }
    }
    assert_eq!(fixture.transport.state.lock().await.native_http.len(), 16);
}

#[tokio::test]
async fn native_conversation_reopen_uses_reserve_without_changing_admitted_identity() {
    let fixture = Fixture::with_capacity(
        Mode::Success,
        NativeRuntimeTransport::KubernetesExec,
        crate::agent_sandbox::RuntimeCapacityConfig {
            stream_reserve: 16,
            request_reserve: 8,
            ..Default::default()
        },
    )
    .await;
    let endpoint = fixture.endpoint().await;
    let held: Vec<_> = (0..48)
        .map(|_| {
            fixture
                .transport
                .runtime_capacity
                .try_acquire(RuntimePermitKind::Stream, RuntimeWorkloadClass::Ordinary)
                .unwrap()
        })
        .collect();
    for kind in [RuntimeStreamKind::Conversation, RuntimeStreamKind::Desktop] {
        assert!(matches!(
            fixture.transport.native_stream(&endpoint, kind).await,
            Err(AdapterError::RuntimeCapacity(_))
        ));
    }
    let stream = fixture
        .transport
        .native_stream(&endpoint, RuntimeStreamKind::ContinuityConversation)
        .await
        .unwrap();
    assert_eq!(fixture.transport.runtime_capacity.available_streams(), 15);
    drop(stream);
    // A nested ordinary transport operation inherits already-admitted recovery.
    let stream = RuntimeWorkloadClass::Continuity
        .scope(
            RuntimeWorkloadClass::Ordinary.scope(
                fixture
                    .transport
                    .native_stream(&endpoint, RuntimeStreamKind::Conversation),
            ),
        )
        .await
        .unwrap();
    drop((stream, held));
    assert_eq!(fixture.transport.runtime_capacity.available_streams(), 64);
}

#[tokio::test]
async fn continuity_and_control_requests_use_reserve_at_full_ordinary_capacity() {
    let fixture = Fixture::with_capacity(
        Mode::Persistent,
        NativeRuntimeTransport::KubernetesExec,
        crate::agent_sandbox::RuntimeCapacityConfig {
            request_reserve: 8,
            ..Default::default()
        },
    )
    .await;
    let endpoint = fixture.endpoint().await;
    let authority = &fixture.transport.runtime_capacity;
    let held: Vec<_> = (0..24)
        .map(|_| {
            authority
                .try_acquire(RuntimePermitKind::Request, RuntimeWorkloadClass::Ordinary)
                .unwrap()
        })
        .collect();
    assert!(matches!(
        fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await,
        Err(AdapterError::RuntimeCapacity(_))
    ));
    for class in [
        RuntimeWorkloadClass::Continuity,
        RuntimeWorkloadClass::Control,
    ] {
        class
            .scope(
                fixture
                    .transport
                    .control(&endpoint, &RequestFrame::Resources),
            )
            .await
            .unwrap();
        assert_eq!(authority.available_requests(), 8);
    }
    drop(held);
    fixture
        .transport
        .control(&endpoint, &RequestFrame::Resources)
        .await
        .unwrap();
    assert_eq!(authority.available_requests(), 32);
}
