use super::*;
use crate::agent_sandbox::RuntimeCapacityConfig;
use crate::owner_protocol::{test_support::TestAuthority, TransportAuthority};
use native_pool::NativeHttpLane::{Control, Ownership};

pub(super) type Requests = tokio::task::JoinSet<crate::Result<ResponseFrame>>;

pub(super) async fn busy_fixture(reserve: usize) -> (Fixture, Vec<RuntimeHandle>, Requests) {
    let fixture = Fixture::with_capacity(
        Mode::Persistent,
        NativeRuntimeTransport::KubernetesExec,
        RuntimeCapacityConfig {
            request_reserve: 8,
            native_http_session_reserve: reserve,
            ..Default::default()
        },
    )
    .await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-reserved", &"a".repeat(64))
        .await
        .unwrap();
    let RuntimeDiscovery::AgentSandbox(client) = &fixture.transport.client else {
        unreachable!()
    };
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let mut held = Requests::new();
    let mut handles = Vec::new();
    let ordinary_limit = 16 - reserve;
    for index in 0..ordinary_limit {
        let name = format!("busy-{index}");
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
        let transport = fixture.transport.clone();
        held.spawn(async move { transport.control(&endpoint, &RequestFrame::Resources).await });
        handles.push(handle);
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.requests.lock().unwrap().len() != ordinary_limit {
            assert!(
                held.try_join_next().is_none(),
                "busy request ended before release"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("ordinary HTTP requests reached the fixture");
    assert_eq!(
        fixture.transport.state.lock().await.native_http.len(),
        ordinary_limit
    );
    assert_eq!(
        fixture.transport.runtime_capacity.available_requests(),
        32 - ordinary_limit
    );
    (fixture, handles, held)
}

pub(super) async fn finish(fixture: &Fixture, handles: Vec<RuntimeHandle>, mut held: Requests) {
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    while let Some(result) = held.join_next().await {
        assert!(matches!(
            result.unwrap().unwrap(),
            ResponseFrame::Resources(_)
        ));
    }
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 32);
    for handle in handles {
        fixture.transport.retire(&handle).await.unwrap();
    }
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

fn assert_capacity(error: AdapterError, class: RuntimeWorkloadClass) {
    let AdapterError::RuntimeCapacity(failure) = error else {
        panic!("expected typed native capacity refusal, got {error:?}");
    };
    assert_eq!(failure.reason, RuntimeCapacityReason::NativeHttpSessions);
    assert_eq!(failure.class, class);
    assert_eq!(failure.kind, RuntimePermitKind::Request);
    assert_eq!(failure.code(), "runtime_request_capacity_exhausted");
}

async fn challenge(
    fixture: &Fixture,
    endpoint: &RuntimeEndpoint,
    lane: native_pool::NativeHttpLane,
    class: RuntimeWorkloadClass,
) -> crate::Result<NativeHttpResponse> {
    class
        .scope(
            fixture.transport.native_http_for(
                endpoint,
                http::Method::POST,
                endpoint
                    .control()
                    .unwrap()
                    .join("/runtime/agents/agent-1/manager-ownership/challenge")
                    .unwrap(),
                serde_json::to_vec(&TestAuthority::new(7).owner()).unwrap(),
                8192,
                (lane, RuntimeWorkloadClass::Ordinary),
            ),
        )
        .await
}

#[tokio::test]
async fn disabled_session_reserve_retains_full_pool_refusal_despite_free_request_permits() {
    let (fixture, handles, held) = busy_fixture(0).await;
    let distinct = fixture.endpoint().await;
    let sibling = fixture.transport.endpoint(&handles[0]).await.unwrap();
    for (endpoint, lane) in [(&distinct, Control), (&sibling, Ownership)] {
        for class in [
            RuntimeWorkloadClass::Continuity,
            RuntimeWorkloadClass::Control,
        ] {
            assert_capacity(
                challenge(&fixture, endpoint, lane, class)
                    .await
                    .err()
                    .unwrap(),
                class,
            );
            assert_eq!(fixture.transport.runtime_capacity.available_requests(), 16);
        }
    }
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 16);
    finish(&fixture, handles, held).await;
}

#[tokio::test]
async fn session_reserve_protects_privileged_requests_on_distinct_runtimes_and_lanes() {
    let (fixture, handles, mut held) = busy_fixture(2).await;
    let distinct = fixture.endpoint().await;
    let sibling = fixture.transport.endpoint(&handles[0]).await.unwrap();
    drop(fixture.transport.native_slot(&sibling).await.unwrap());
    for (endpoint, lane) in [(&distinct, Control), (&sibling, Ownership)] {
        assert_capacity(
            fixture
                .transport
                .native_slot_for(endpoint, lane)
                .await
                .err()
                .expect("ordinary requests must leave reserved sessions free"),
            RuntimeWorkloadClass::Ordinary,
        );
        assert_capacity(
            challenge(&fixture, endpoint, lane, RuntimeWorkloadClass::Ordinary)
                .await
                .err()
                .unwrap(),
            RuntimeWorkloadClass::Ordinary,
        );
        assert_eq!(fixture.transport.runtime_capacity.available_requests(), 18);
        for class in [
            RuntimeWorkloadClass::Continuity,
            RuntimeWorkloadClass::Control,
        ] {
            let response = tokio::time::timeout(
                Duration::from_secs(2),
                challenge(&fixture, endpoint, lane, class),
            )
            .await
            .expect("privileged request must finish while ordinary bodies remain held")
            .unwrap();
            assert!(response.status.is_success());
            let _: crate::owner_protocol::OwnerChallenge =
                serde_json::from_slice(&response.bytes).unwrap();
            response.recycle().await.unwrap();
            assert_eq!(fixture.transport.runtime_capacity.available_requests(), 18);
            assert!(
                held.try_join_next().is_none(),
                "ordinary work was interrupted"
            );
        }
    }
    let first = challenge(
        &fixture,
        &distinct,
        Control,
        RuntimeWorkloadClass::Continuity,
    )
    .await
    .unwrap();
    let second = challenge(&fixture, &sibling, Ownership, RuntimeWorkloadClass::Control)
        .await
        .unwrap();
    assert_eq!(fixture.transport.state.lock().await.native_http.len(), 16);
    for class in [
        RuntimeWorkloadClass::Continuity,
        RuntimeWorkloadClass::Control,
    ] {
        assert_capacity(
            challenge(&fixture, &distinct, Ownership, class)
                .await
                .err()
                .unwrap(),
            class,
        );
    }
    // Saturated other lanes also bound reuse of an already-busy ordinary lane.
    for endpoint in [&distinct, &sibling] {
        assert_capacity(
            fixture
                .transport
                .native_slot(endpoint)
                .await
                .err()
                .expect("ordinary reuse cannot borrow occupied reserve"),
            RuntimeWorkloadClass::Ordinary,
        );
    }
    first.recycle().await.unwrap();
    let third = challenge(
        &fixture,
        &distinct,
        Ownership,
        RuntimeWorkloadClass::Control,
    )
    .await
    .unwrap();
    assert!(third.status.is_success());
    assert!(
        held.try_join_next().is_none(),
        "idle eviction must preserve active sessions"
    );
    second.recycle().await.unwrap();
    third.recycle().await.unwrap();
    finish(&fixture, handles, held).await;
}

#[tokio::test]
async fn session_reserve_releases_cancelled_and_panicked_admission_without_evicting_busy_work() {
    let (fixture, handles, mut held) = busy_fixture(15).await;
    let endpoint = fixture.endpoint().await;
    let busy = fixture.transport.endpoint(&handles[0]).await.unwrap();
    let mut waiting = Box::pin(
        RuntimeWorkloadClass::Control
            .scope(fixture.transport.control(&busy, &RequestFrame::Resources)),
    );
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 30);
    drop(waiting);
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 31);
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        RuntimeWorkloadClass::Continuity
            .scope(fixture.transport.control(&busy, &RequestFrame::Resources),)
    )
    .await
    .is_err());
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 31);
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
    assert_capacity(
        fixture
            .transport
            .native_slot(&endpoint)
            .await
            .err()
            .unwrap(),
        RuntimeWorkloadClass::Ordinary,
    );

    held.abort_all();
    while let Some(result) = held.join_next().await {
        assert!(result.unwrap_err().is_cancelled());
    }
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 32);
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    fixture.drained().await;
    let slot = fixture.transport.native_slot(&endpoint).await.unwrap();
    let lease = fixture
        .transport
        .native_checkout(&endpoint, slot)
        .await
        .unwrap();
    let permit = fixture
        .transport
        .runtime_capacity
        .try_acquire(RuntimePermitKind::Request, RuntimeWorkloadClass::Ordinary)
        .unwrap();
    let panic = tokio::spawn(async move {
        let (_lease, _permit) = (lease, permit);
        panic!("test-only native admission unwind");
    });
    assert!(panic.await.unwrap_err().is_panic());
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 32);
    fixture
        .transport
        .control(&endpoint, &RequestFrame::Resources)
        .await
        .unwrap();
    // Cached connections still need admission when another lane is busy.
    let slot = fixture
        .transport
        .native_slot_for_class(&busy, Ownership, RuntimeWorkloadClass::Control)
        .await
        .unwrap();
    assert_capacity(
        fixture
            .transport
            .native_slot(&endpoint)
            .await
            .err()
            .unwrap(),
        RuntimeWorkloadClass::Ordinary,
    );
    drop(slot);
    fixture
        .transport
        .control(&endpoint, &RequestFrame::Resources)
        .await
        .unwrap();
    finish(&fixture, handles, held).await;
}

#[tokio::test]
async fn session_reserve_preserves_same_lane_serialization_and_fresh_identity() {
    let (fixture, handles, held) = busy_fixture(2).await;
    let busy = fixture.transport.endpoint(&handles[0]).await.unwrap();
    let mut waiting = Box::pin(
        RuntimeWorkloadClass::Control
            .scope(fixture.transport.control(&busy, &RequestFrame::Resources)),
    );
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 14);
    let endpoint = fixture.endpoint().await;
    challenge(
        &fixture,
        &endpoint,
        Control,
        RuntimeWorkloadClass::Continuity,
    )
    .await
    .unwrap()
    .recycle()
    .await
    .unwrap();
    let before = fixture.state.requests.lock().unwrap().len();
    fixture.state.restarted.store(true, Ordering::SeqCst);
    assert!(matches!(
        challenge(&fixture, &endpoint, Control, RuntimeWorkloadClass::Control).await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(
        fixture.state.requests.lock().unwrap().len(),
        before,
        "fresh Pod proof must precede reserved request bytes"
    );
    fixture.state.restarted.store(false, Ordering::SeqCst);
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap(),
        ResponseFrame::Resources(_)
    ));
    assert_eq!(
        fixture.state.requests.lock().unwrap().len(),
        before + 1,
        "same-lane request sends once after checkout"
    );
    finish(&fixture, handles, held).await;
}

#[tokio::test]
async fn session_reserve_reports_refusal_with_only_bounded_metric_labels() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recorder = metrics::set_default_local_recorder(&recorder);
    let (fixture, handles, held) = busy_fixture(15).await;
    let endpoint = fixture.endpoint().await;
    assert_capacity(
        fixture
            .transport
            .native_slot(&endpoint)
            .await
            .err()
            .unwrap(),
        RuntimeWorkloadClass::Ordinary,
    );
    let text = metrics.render();
    assert!(text.contains("cma_adapter_runtime_refusals_total{kind=\"request\",class=\"ordinary\",reason=\"native_http_sessions\"} 1"));
    assert!(!text.contains("busy-0"));
    assert!(!text.contains("sandbox-reserved"));
    finish(&fixture, handles, held).await;
}

#[tokio::test]
async fn session_reserve_protects_the_portforward_ownership_lane() {
    let fixture = Fixture::with_capacity(
        Mode::Persistent,
        NativeRuntimeTransport::KubernetesPortforward,
        RuntimeCapacityConfig {
            native_http_session_reserve: 15,
            request_reserve: 8,
            ..Default::default()
        },
    )
    .await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-forwarded", &"a".repeat(64))
        .await
        .unwrap();
    let ordinary = fixture.endpoint().await;
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let transport = fixture.transport.clone();
    let mut held = Requests::new();
    held.spawn(async move { transport.control(&ordinary, &RequestFrame::Resources).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.requests.lock().unwrap().len() != 1 {
            assert!(held.try_join_next().is_none());
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let endpoint = fixture.endpoint().await;
    assert_capacity(
        challenge(
            &fixture,
            &endpoint,
            Ownership,
            RuntimeWorkloadClass::Ordinary,
        )
        .await
        .err()
        .unwrap(),
        RuntimeWorkloadClass::Ordinary,
    );
    for class in [
        RuntimeWorkloadClass::Continuity,
        RuntimeWorkloadClass::Control,
    ] {
        let response = challenge(&fixture, &endpoint, Ownership, class)
            .await
            .unwrap();
        assert!(response.status.is_success());
        response.recycle().await.unwrap();
        assert!(held.try_join_next().is_none());
    }
    assert_eq!(fixture.transport.state.lock().await.native_http.len(), 2);
    finish(&fixture, vec![], held).await;
}
