use super::*;
use crate::agent_sandbox::{RuntimeCapacityReason, RuntimePermitKind, RuntimeWorkloadClass};
use crate::hosted::HostedControlTransport;

#[path = "native_close_effect_tests.rs"]
mod close_effect;

#[path = "native_close_lane_tests.rs"]
mod close_lane;

#[path = "native_http_reserve_tests.rs"]
mod session_reserve;

#[path = "native_http_pool/telemetry/tests.rs"]
mod telemetry_tests;

#[tokio::test]
async fn native_pool_measurements_distinguish_idle_cache_from_busy_checkout() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recorder = metrics::set_default_local_recorder(&recorder);
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-metrics", &"a".repeat(64))
        .await
        .unwrap();
    let endpoint = fixture.endpoint().await;
    for _ in 0..2 {
        fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .unwrap();
    }
    let rendered = metrics.render();
    for (kind, value) in [
        ("slots", 1),
        ("cached_connections", 1),
        ("busy", 0),
        ("waiting", 0),
    ] {
        assert!(
            rendered.contains(&format!(
                "cma_adapter_native_http_pool{{pool=\"native\",kind=\"{kind}\"}} {value}"
            )),
            "{rendered}"
        );
    }
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 32);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    let rendered = metrics.render();
    for kind in ["slots", "cached_connections", "busy", "waiting"] {
        assert!(
            rendered.contains(&format!(
                "cma_adapter_native_http_pool{{pool=\"native\",kind=\"{kind}\"}} 0"
            )),
            "{rendered}"
        );
    }
    assert!(!rendered.contains("sandbox-metrics"));
    assert!(!rendered.contains("agent-sandbox:"));
}

#[tokio::test]
async fn session_reserve_does_not_bypass_a_busy_control_lane_for_authorized_close() {
    use crate::owner_protocol::{scope, test_support::TestAuthority, TransportAuthority};

    let (fixture, handles, mut held) = session_reserve::busy_fixture(2).await;
    let authority = TestAuthority::new(7);
    let request = RequestFrame::CloseAgent(crate::wire::FencedRequest::new(
        7,
        crate::AgentCloseRequest::new("close-held-native-agent", crate::AgentSlot::new("agent-1")),
    ));
    let mut close = Box::pin(scope(
        authority.clone(),
        fixture.transport.exchange(&handles[0], request),
    ));
    // This records a pre-existing activation limit, not a control-SLO pass:
    // ownership negotiation succeeds, but CloseAgent still waits on Control.
    assert!(tokio::time::timeout(Duration::from_secs(1), close.as_mut())
        .await
        .is_err());
    assert!(authority.is_current());
    assert!(fixture.state.ownership_confirmations.load(Ordering::SeqCst) > 0);
    assert!(held.try_join_next().is_none());
    assert!(!fixture
        .state
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.contains("close-held-native-agent")));
    drop(close);
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 18);
    session_reserve::finish(&fixture, handles, held).await;
}

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
