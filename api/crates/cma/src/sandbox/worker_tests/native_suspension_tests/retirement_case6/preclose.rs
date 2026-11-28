use super::*;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use layer2_core::agent_ownership::test_support::MemoryOwnershipStore;
use layer3_adapter::agent_sandbox::{AgentSandboxClient, AgentSandboxConfig};
use layer3_adapter::hosted::{NativeRuntimeTransport, ProviderHostedControlTransport};
use layer3_adapter::owner_protocol::{OwnershipFailure, OwnershipPhase, TransportOwner};
use layer3_adapter::receiver_failure::{self, ChallengeRefusal};

#[tokio::test]
async fn owned_preclose_challenge_refusal_seals_capture_without_dispatching_close() {
    let (f, active, children, mut now) = active_siblings().await;
    let clock = Arc::new(Mutex::new(now));
    let parent = finalizing_parent_worker(&f, clock.clone());
    let coordinator = OwnershipCoordinator::register(
        Arc::new(MemoryOwnershipStore::default()),
        Member {
            instance_id: "preclose-worker".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 2,
        },
    )
    .await
    .unwrap();
    let mut config =
        AgentWorkerConfig::new("preclose-worker", "test").with_ownership(Some(coordinator.clone()));
    config.retirement_reporter = Some(f.backend.retirement_records.clone());
    let child = configured_child_worker(&f, config);
    admit(&f, &active).await;
    now += Duration::from_secs(6);
    *clock.lock().unwrap() = now + Duration::from_secs(1);
    parent.run_one(&active.id, now).await.unwrap();
    let agent = &children[0];
    assert!(child.run_one(&agent.id, now).await.unwrap());
    let stored = f.harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stored.work.step, AgentWorkStep::CloseProcess);
    let authority = coordinator.acquire(&agent.id).await.unwrap().unwrap();

    let (transport, exchanges, server) = receiver().await;
    *f.backend.inspection_transport.lock().unwrap() = Some(transport.clone());
    *f.backend.close_transport.lock().unwrap() = Some(transport);
    let (capture, attempt) = receiver_failure::capture(child.run_one(&agent.id, now));
    let result = attempt.await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());

    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert_eq!(&*exchanges.lock().unwrap(), &["challenge"]);
    assert_eq!(
        authority.receiver_failure(),
        Some(OwnershipFailure::http(OwnershipPhase::Challenge, 404))
    );
    let evidence = capture
        .sealed()
        .expect("owned inspection seals the refusal");
    assert_eq!(evidence.refusal(), ChallengeRefusal::NotFound);
    let receiver = evidence.receiver();
    assert_eq!(
        receiver.runtime.as_str(),
        active.runtime.as_ref().unwrap().runtime_id
    );
    assert_eq!(receiver.pod, records::pod());
    assert_eq!(receiver.owner.agent_id, agent.id.as_str());
    assert_eq!(receiver.owner.epoch, authority.token.epoch);
    assert_eq!(receiver.owner.instance_id, authority.token.instance_id);
    assert!(!f
        .backend
        .calls
        .snapshot()
        .iter()
        .any(|call| call == "receiver-close-attempt"));
    blocked(&f, &active, std::slice::from_ref(agent)).await;
    assert!(
        f.backend
            .retirement_records
            .observation
            .lock()
            .unwrap()
            .is_none(),
        "the uncomposed capture primitive cannot write through the legacy reporter"
    );
}

type ReceiverFixture = (
    Arc<ProviderHostedControlTransport>,
    Arc<Mutex<Vec<&'static str>>>,
    tokio::task::JoinHandle<()>,
);

async fn receiver() -> ReceiverFixture {
    receiver_with_status("404 Not Found").await
}

async fn receiver_with_status(response: &'static str) -> ReceiverFixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let exchanges = Arc::new(Mutex::new(Vec::new()));
    let router = axum::Router::new()
        .route("/apis/agents.x-k8s.io/v1beta1/namespaces/fixture/sandboxes/retained-crd", axum::routing::get(|| async {
            axum::Json(json!({
                "apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
                "metadata":{"name":"retained-crd","namespace":"fixture","uid":"original-uid",
                    "labels":{"cma/control-plane":"fixture"},"generation":1,"resourceVersion":"1"},
                "status":{"conditions":[{"type":"Ready","status":"True","observedGeneration":1}]}
            }))
        }))
        .route("/api/v1/namespaces/fixture/pods", axum::routing::get(|| async {
            let tracking = b"retained-crd".iter().fold(2_166_136_261u32, |hash, byte| {
                (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
            });
            let pod = records::pod();
            axum::Json(json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[{
                "metadata":{"name":"fixture-pod","namespace":"fixture","uid":pod.uid,
                    "labels":{"cma/control-plane":"fixture","agents.x-k8s.io/sandbox-name-hash":format!("{tracking:08x}")},
                    "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
                        "name":"retained-crd","uid":"original-uid","controller":true}]},
                "status":{"phase":"Running","podIP":"192.0.2.1","conditions":[{"type":"Ready","status":"True"}],
                    "containerStatuses":[{"name":"runtime","image":"fixture","imageID":"fixture",
                        "containerID":pod.container_id,"ready":true,"restartCount":pod.restart_count,"state":{"running":{}}}]}
            }]}))
        }))
        .route(
            "/api/v1/namespaces/fixture/pods/fixture-pod/portforward",
            axum::routing::get(
                move |ws: WebSocketUpgrade, State(exchanges): State<Arc<Mutex<Vec<&'static str>>>>| async move {
                    ws.protocols(["v5.channel.k8s.io"])
                        .on_upgrade(move |socket| refuse_challenge(socket, exchanges, response))
                },
            ),
        )
        .with_state(exchanges.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap(),
        AgentSandboxConfig {
            namespace: "fixture".into(),
            deployment: "fixture".into(),
            control_key: "fixture".repeat(8),
            runtime_class: None,
            service_account: "default".into(),
            image_pull_secrets: Vec::new(),
            template_network_policy: None,
            template_fs_group_change_policy: None,
            native_claim_guard_enabled: false,
            runtime_startup_breaker_enabled: false,
        },
        "fixture",
    )
    .unwrap();
    let transport = ProviderHostedControlTransport::for_agent_sandbox_with_transport(
        client,
        NativeRuntimeTransport::KubernetesPortforward,
    )
    .unwrap();
    (Arc::new(transport), exchanges, server)
}

async fn refuse_challenge(
    mut socket: WebSocket,
    exchanges: Arc<Mutex<Vec<&'static str>>>,
    response: &str,
) {
    for channel in [0, 1] {
        let mut frame = vec![channel];
        frame.extend_from_slice(&9090_u16.to_le_bytes());
        socket.send(Message::Binary(frame.into())).await.unwrap();
    }
    let mut request = Vec::new();
    let owner = loop {
        let Message::Binary(frame) = socket.recv().await.unwrap().unwrap() else {
            panic!("expected native HTTP bytes");
        };
        assert_eq!(frame[0], 0);
        request.extend_from_slice(&frame[1..]);
        assert!(request.len() < 8192);
        if let Some(offset) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            if let Ok(owner) = serde_json::from_slice::<TransportOwner>(&request[offset + 4..]) {
                break owner;
            }
        }
    };
    owner.validate().unwrap();
    let expected = format!(
        "POST /runtime/agents/{}/manager-ownership/challenge ",
        owner.agent_id
    );
    assert!(request.starts_with(expected.as_bytes()));
    exchanges.lock().unwrap().push("challenge");
    let response = format!("\x00HTTP/1.1 {response}\r\nContent-Length: 0\r\n\r\n");
    socket
        .send(Message::Binary(response.into_bytes().into()))
        .await
        .unwrap();
    while socket.recv().await.is_some_and(|frame| frame.is_ok()) {}
}

pub(in crate::sandbox::worker_tests) mod mongo;
