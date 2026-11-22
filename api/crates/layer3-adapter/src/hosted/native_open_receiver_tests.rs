//! Exercise lease reuse through real Kubernetes HTTP and conversation streams.
use super::*;
use crate::owner_protocol::{scope, test_support::TestAuthority, TransportAuthority};

fn authority() -> Arc<TestAuthority> {
    let mut authority = TestAuthority::new(7);
    Arc::get_mut(&mut authority).unwrap().owner.agent_id = "agent-a".into();
    authority
}

async fn setup(mode: NativeRuntimeTransport) -> Fixture {
    let fixture = Fixture::start(Mode::Persistent, mode).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-one", &"a".repeat(64))
        .await
        .unwrap();
    fixture
}

fn opens(fixture: &Fixture) -> usize {
    fixture
        .state
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.starts_with("POST /runtime/control "))
        .count()
}

async fn wait_for_open(fixture: &Fixture) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while opens(fixture) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Open reached the native stream");
}

#[tokio::test]
async fn native_open_uses_one_receiver_negotiation_and_one_effect() {
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        let fixture = setup(mode).await;
        let endpoint = fixture.endpoint().await;
        let authority = authority();
        let (receipt, socket, lease) = scope(authority.clone(), async {
            let lease = fixture.transport.receiver_lease(&endpoint).await?.unwrap();
            let (receipt, socket) = fixture
                .transport
                .open_and_connect(
                    &handle(),
                    &endpoint,
                    &launch(),
                    Some(&lease),
                    &mut crate::hosted::startup_timing::StartupTiming::new(),
                    RuntimeWorkloadClass::Ordinary,
                )
                .await?;
            Ok::<_, AdapterError>((receipt, socket, lease))
        })
        .await
        .unwrap();
        assert_eq!(receipt.authority_fence, launch().authority_fence);
        assert_eq!(receipt.receipt.operation_id, launch().request.operation_id);
        assert_eq!(authority.renewals.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.state.ownership_confirmations.load(Ordering::SeqCst),
            1
        );
        assert_eq!(opens(&fixture), 1);
        drop((socket, lease));
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn held_receiver_rejects_other_authority_agent_endpoint_and_transport_before_bytes() {
    let fixture = setup(NativeRuntimeTransport::KubernetesExec).await;
    let endpoint = fixture.endpoint().await;
    let authority = authority();
    let lease = scope(
        authority.clone(),
        fixture.transport.receiver_lease(&endpoint),
    )
    .await
    .unwrap()
    .unwrap();
    let before = fixture.state.requests.lock().unwrap().len();
    assert!(fixture
        .transport
        .open_on_receiver_lease(&endpoint, &launch(), &lease)
        .await
        .is_err());
    for epoch in [7, 8] {
        let mut different = TestAuthority::new(epoch);
        Arc::get_mut(&mut different).unwrap().owner.agent_id = "agent-a".into();
        assert!(scope(
            different,
            fixture
                .transport
                .open_on_receiver_lease(&endpoint, &launch(), &lease)
        )
        .await
        .is_err());
    }
    let mut wrong_agent = launch();
    wrong_agent.request.agent_id = AgentSlot::new("another-agent");
    assert!(scope(
        authority.clone(),
        fixture
            .transport
            .open_on_receiver_lease(&endpoint, &wrong_agent, &lease)
    )
    .await
    .is_err());
    let (runtime, binding) = endpoint.native_binding().unwrap();
    let mut changed_pod = binding.clone();
    changed_pod.pod_uid = "replacement-pod".into();
    let mut restarted = binding.clone();
    restarted.runtime_container_id = Some("replacement-container".into());
    restarted.runtime_restart_count = Some(1);
    let mut changed_identity = endpoint.native_identity().unwrap().clone();
    changed_identity.epoch += 1;
    let changed = [
        RuntimeEndpoint::parse("http://runtime.invalid", "http://127.0.0.1:1").unwrap(),
        endpoint
            .clone()
            .with_control_token("another-control-token".into()),
        endpoint
            .clone()
            .with_native_binding(runtime.clone(), changed_pod),
        endpoint
            .clone()
            .with_native_binding(runtime.clone(), restarted),
        endpoint
            .clone()
            .with_native_binding(RuntimeHandle::new("another-runtime"), binding.clone()),
        endpoint.clone().with_native_identity(changed_identity),
    ];
    for endpoint in changed {
        assert!(scope(
            authority.clone(),
            fixture
                .transport
                .open_on_receiver_lease(&endpoint, &launch(), &lease)
        )
        .await
        .is_err());
    }
    let mut other_transport = (*fixture.transport).clone();
    other_transport.state = Arc::new(Mutex::new(TransportState::default()));
    assert!(scope(
        authority.clone(),
        other_transport.open_on_receiver_lease(&endpoint, &launch(), &lease)
    )
    .await
    .is_err());
    authority.revoke();
    assert!(scope(
        authority,
        fixture
            .transport
            .open_on_receiver_lease(&endpoint, &launch(), &lease)
    )
    .await
    .is_err());
    assert_eq!(fixture.state.requests.lock().unwrap().len(), before);
    drop(lease);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn reused_receiver_keeps_fresh_native_pod_proof_before_effect() {
    for restart in [false, true] {
        let fixture = setup(NativeRuntimeTransport::KubernetesExec).await;
        let endpoint = fixture.endpoint().await;
        let authority = authority();
        let lease = scope(
            authority.clone(),
            fixture.transport.receiver_lease(&endpoint),
        )
        .await
        .unwrap()
        .unwrap();
        if restart {
            fixture.state.restarted.store(true, Ordering::SeqCst);
        } else {
            fixture.state.replaced.store(true, Ordering::SeqCst);
        }
        assert!(scope(
            authority,
            fixture
                .transport
                .open_on_receiver_lease(&endpoint, &launch(), &lease)
        )
        .await
        .is_err());
        assert_eq!(opens(&fixture), 0);
        drop(lease);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn held_open_still_renews_and_refuses_reply_after_revocation() {
    for failure in 0..3 {
        let fixture = setup(NativeRuntimeTransport::KubernetesExec).await;
        fixture.state.hold_response.store(true, Ordering::SeqCst);
        let authority = authority();
        let opening = tokio::spawn({
            let transport = fixture.transport.clone();
            let authority = authority.clone();
            async move {
                scope(authority, async {
                    let endpoint = transport.endpoint(&handle()).await?;
                    let lease = transport.receiver_lease(&endpoint).await?.unwrap();
                    transport
                        .open_on_receiver_lease(&endpoint, &launch(), &lease)
                        .await
                })
                .await
            }
        });
        wait_for_open(&fixture).await;
        assert_eq!(
            fixture.state.ownership_confirmations.load(Ordering::SeqCst),
            1
        );
        if failure == 1 {
            authority.revoke();
        } else if failure == 2 {
            authority.fail_renewal.store(true, Ordering::SeqCst);
            tokio::time::timeout(Duration::from_secs(5), authority.revoked())
                .await
                .expect("failed durable renewal must revoke the held lease");
        } else {
            tokio::time::timeout(Duration::from_secs(5), async {
                while fixture.state.ownership_confirmations.load(Ordering::SeqCst) < 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("renewal remains live during a slow Open");
            assert!(authority.is_current());
        }
        fixture.state.hold_response.store(false, Ordering::SeqCst);
        let result = tokio::time::timeout(Duration::from_secs(2), opening)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.is_err(), failure != 0);
        assert_eq!(opens(&fixture), 1);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn reused_receiver_lost_reply_and_wrong_fence_never_replay_or_connect() {
    for fault in [1, 7] {
        let fixture = setup(NativeRuntimeTransport::KubernetesExec).await;
        let endpoint = fixture.endpoint().await;
        let authority = authority();
        let result = scope(authority, async {
            let lease = fixture.transport.receiver_lease(&endpoint).await?.unwrap();
            fixture.state.fault.store(fault, Ordering::SeqCst);
            fixture
                .transport
                .open_and_connect(
                    &handle(),
                    &endpoint,
                    &launch(),
                    Some(&lease),
                    &mut crate::hosted::startup_timing::StartupTiming::new(),
                    RuntimeWorkloadClass::Ordinary,
                )
                .await
        })
        .await;
        assert!(result.is_err());
        assert_eq!(opens(&fixture), 1);
        assert!(fixture
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.to_ascii_lowercase().contains("upgrade: websocket")));
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn cancelling_open_drops_its_only_renewal_and_never_replays() {
    let fixture = setup(NativeRuntimeTransport::KubernetesExec).await;
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let authority = authority();
    let opening = tokio::spawn({
        let transport = fixture.transport.clone();
        let authority = authority.clone();
        async move {
            scope(authority, async {
                let endpoint = transport.endpoint(&handle()).await?;
                let lease = transport.receiver_lease(&endpoint).await?.unwrap();
                transport
                    .open_and_connect(
                        &handle(),
                        &endpoint,
                        &launch(),
                        Some(&lease),
                        &mut crate::hosted::startup_timing::StartupTiming::new(),
                        RuntimeWorkloadClass::Ordinary,
                    )
                    .await
            })
            .await
        }
    });
    wait_for_open(&fixture).await;
    opening.abort();
    let Err(cancelled) = opening.await else {
        panic!("Open was not cancelled");
    };
    assert!(cancelled.is_cancelled());
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(3300)).await;
    assert_eq!(authority.renewals.load(Ordering::SeqCst), 1);
    assert_eq!(opens(&fixture), 1);
    assert!(fixture
        .state
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| !request.to_ascii_lowercase().contains("upgrade: websocket")));
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}
