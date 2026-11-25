use super::*;
use crate::hosted::{NativeRuntimeTransport, ProviderHostedControlTransport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore};

struct StreamState {
    requests: AtomicUsize,
    received: Notify,
    gate: Semaphore,
    fault: AtomicUsize,
    replace_volume: AtomicBool,
    owner: Mutex<Option<crate::owner_protocol::TransportOwner>>,
    owner_events: Mutex<Vec<&'static str>>,
    refuse_confirmation: AtomicBool,
}

struct ReusedInspection {
    bound: Bound,
    adapter: Arc<CodexAdapter>,
    backend: Arc<AgentSandboxBackend>,
    transport: Arc<ProviderHostedControlTransport>,
    adoption: RuntimeAdoptionRequest,
    home: HomeVolumeAttachment,
    stream: Arc<StreamState>,
    task: tokio::task::JoinHandle<()>,
    ownership_task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for ReusedInspection {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(task) = &self.ownership_task {
            task.abort();
        }
    }
}

async fn fixture(explicit_policy: bool, shared_client: bool) -> ReusedInspection {
    let bound = bound(explicit_policy).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    let backend_client = client(&bound.fixture, false);
    let transport_client = if shared_client {
        backend_client.clone()
    } else {
        client(&bound.fixture, false)
    };
    let transport = Arc::new(
        ProviderHostedControlTransport::for_agent_sandbox_with_transport(
            transport_client,
            NativeRuntimeTransport::KubernetesExec,
        )
        .unwrap(),
    );
    let backend = Arc::new(AgentSandboxBackend::new(backend_client, transport.clone()));
    backend.provision(bound.request.clone()).await.unwrap();
    let config = super::super::super::super::tests::spec();
    let adapter = Arc::new(
        CodexAdapter::new(
            backend.clone(),
            CodexSandboxSpec {
                deployment: HostedDeployment {
                    image: config.image.clone(),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .with_home_volumes(Arc::new(
            NativeHomeVolumes::new(
                bound.fixture.kubernetes.clone(),
                bound.fixture.client.config.clone(),
                "fixture",
            )
            .unwrap(),
        )),
    );
    let adoption = RuntimeAdoptionRequest::new(
        1,
        bound.runtime.clone(),
        SandboxSpec {
            sandbox_id: home.request.sandbox_id.clone(),
            runtime_image: Some(config.image.uri),
            created_by: Some("cma-test".into()),
            home_volume: Some(Box::new(home.clone())),
            capacity: SandboxCapacity {
                cpu_millis: config.cpu_millis,
                memory_mib: config.memory_mib,
            },
            ..Default::default()
        },
    );
    let socket = transport
        .seed_native_status_connection(&bound.runtime)
        .await;
    let stream = Arc::new(StreamState {
        requests: AtomicUsize::new(0),
        received: Notify::new(),
        gate: Semaphore::new(1),
        fault: AtomicUsize::new(0),
        replace_volume: AtomicBool::new(false),
        owner: Mutex::new(None),
        owner_events: Mutex::new(Vec::new()),
        refuse_confirmation: AtomicBool::new(false),
    });
    let task = tokio::spawn(serve(socket, stream.clone(), bound.fixture.state.clone()));
    bound.fixture.state.lock().unwrap().calls.clear();
    ReusedInspection {
        bound,
        adapter,
        backend,
        transport,
        adoption,
        home,
        stream,
        task,
        ownership_task: None,
    }
}

async fn serve(
    mut socket: tokio::io::DuplexStream,
    state: Arc<StreamState>,
    authority: Arc<Mutex<State>>,
) {
    loop {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let Ok(byte) = socket.read_u8().await else {
                return;
            };
            bytes.push(byte);
            assert!(bytes.len() < 8192);
        }
        let headers = String::from_utf8(bytes).unwrap();
        assert!(headers.to_lowercase().contains("x-cma-runtime-token:"));
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())
            })
            .unwrap();
        let mut bytes = vec![0; length];
        socket.read_exact(&mut bytes).await.unwrap();
        let owner = state.owner.lock().unwrap().clone();
        if let Some(owner) = owner {
            let action = headers.split_whitespace().nth(1).unwrap();
            if action.ends_with("/manager-ownership/challenge") {
                state.owner_events.lock().unwrap().push("challenge");
                let body = serde_json::to_vec(&json!({
                    "protocol": crate::owner_protocol::OWNER_PROTOCOL_VERSION,
                    "nonce": uuid::Uuid::new_v4(),
                    "valid_for_ms": crate::owner_protocol::RECEIVER_LEASE_MS,
                }))
                .unwrap();
                write_owner_response(&mut socket, &body).await;
                continue;
            }
            if action.ends_with("/manager-ownership/confirm") {
                state.owner_events.lock().unwrap().push("confirm");
                let confirmation: crate::owner_protocol::OwnerConfirmation =
                    serde_json::from_slice(&bytes).unwrap();
                assert_eq!(confirmation.owner, owner);
                let body = if state.refuse_confirmation.load(Ordering::SeqCst) {
                    serde_json::to_vec(&json!({"protocol": 0, "owner": owner})).unwrap()
                } else {
                    serde_json::to_vec(&json!({
                        "protocol": crate::owner_protocol::OWNER_PROTOCOL_VERSION,
                        "owner": owner,
                    }))
                    .unwrap()
                };
                write_owner_response(&mut socket, &body).await;
                continue;
            }
            assert!(headers.contains(&owner.header().unwrap()));
            assert_eq!(
                &*state.owner_events.lock().unwrap(),
                &["challenge", "confirm"],
                "owned status must negotiate before sending the status request"
            );
            state.owner_events.lock().unwrap().push("status");
        }
        assert!(headers.starts_with("POST /runtime/control HTTP/1.1\r\n"));
        let RequestFrame::RuntimeStatus(request) = serde_json::from_slice(&bytes).unwrap() else {
            panic!("only status may consume this operation proof")
        };
        state.requests.fetch_add(1, Ordering::SeqCst);
        state.received.notify_waiters();
        let _permit = state.gate.acquire().await.unwrap();
        match state.fault.swap(0, Ordering::SeqCst) {
            1 => return,
            2 => {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}")
                    .await
                    .unwrap();
                return;
            }
            _ => {}
        }
        if state.replace_volume.load(Ordering::SeqCst) {
            authority.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] =
                json!("changed-after-status");
        }
        let body = serde_json::to_vec(&ResponseFrame::RuntimeStatus(RuntimeStatus {
            agent_filesystem: false,
            agent_attachment_upload: false,
            agent_skill_install: true,
            agent_skill_upgrade: false,
            agent_model_catalog: true,
            agent_browser: false,
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
            sandbox_id: request.sandbox_id,
            generation: request.generation,
            home_identity: request.home_identity,
            boot_id: "a".repeat(32),
            installation_ready: true,
            credential_ready: true,
            restore_pending: false,
            agent_processes: 0,
        }))
        .unwrap();
        if socket
            .write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
            )
            .await
            .is_err()
            || socket.write_all(&body).await.is_err()
        {
            return;
        }
    }
}

fn reads(fixture: &ReusedInspection) -> usize {
    let state = fixture.bound.fixture.state.lock().unwrap();
    assert!(state
        .calls
        .iter()
        .all(|(method, _)| method.starts_with("GET ")));
    state.calls.len()
}

async fn received(fixture: &ReusedInspection) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let notified = fixture.stream.received.notified();
            if fixture.stream.requests.load(Ordering::SeqCst) != 0 {
                return;
            }
            notified.await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_status_reuses_only_call_local_proof_and_keeps_post_status_home_reads() {
    for explicit_policy in [false, true] {
        let fixture = fixture(explicit_policy, true).await;
        for expected_calls in 1..=2 {
            fixture.bound.fixture.state.lock().unwrap().calls.clear();
            let result = fixture
                .adapter
                .inspect_runtime(&fixture.adoption)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.observation.handle, fixture.bound.runtime);
            assert!(result.status.unwrap().credential_ready);
            assert_eq!(reads(&fixture), if explicit_policy { 15 } else { 11 });
            assert_eq!(
                fixture.stream.requests.load(Ordering::SeqCst),
                expected_calls
            );
        }
    }
}

#[tokio::test]
async fn native_status_separate_client_keeps_original_dispatch_authority_reads() {
    let fixture = fixture(false, false).await;
    fixture
        .adapter
        .inspect_runtime(&fixture.adoption)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reads(&fixture), 13);
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_status_queued_request_proves_authority_after_checkout() {
    let fixture = fixture(false, true).await;
    let held = fixture.stream.gate.acquire().await.unwrap();
    let adapter = fixture.adapter.clone();
    let adoption = fixture.adoption.clone();
    let first = tokio::spawn(async move { adapter.inspect_runtime(&adoption).await });
    received(&fixture).await;
    fixture.bound.fixture.state.lock().unwrap().calls.clear();
    let second = fixture.adapter.inspect_runtime(&fixture.adoption);
    tokio::pin!(second);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut second)
            .await
            .is_err()
    );
    assert_eq!(
        reads(&fixture),
        0,
        "queued status must not collect an earlier proof"
    );
    fixture
        .bound
        .fixture
        .edit("sandboxclaims", &fixture.bound.claim.name, |claim| {
            claim["metadata"]["uid"] = json!("replacement");
        });
    drop(held);
    assert!(
        first.await.unwrap().is_err(),
        "first call still checks home after reply"
    );
    assert!(second.await.is_err());
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_status_rejects_changed_claim_volume_policy_and_process_before_bytes() {
    for change in ["claim", "volume", "policy", "pod", "container"] {
        let fixture = fixture(change == "policy", true).await;
        match change {
            "claim" => {
                fixture
                    .bound
                    .fixture
                    .edit("sandboxclaims", &fixture.bound.claim.name, |v| {
                        v["metadata"]["uid"] = json!("replacement");
                    })
            }
            "policy" => fixture.bound.fixture.edit(
                "sandboxtemplates",
                &fixture.bound.claim.request.pool.template_name,
                |v| {
                    v["spec"]["networkPolicy"]["egress"] = json!([{}]);
                },
            ),
            _ => {
                let mut state = fixture.bound.fixture.state.lock().unwrap();
                let (path, pointer) = match change {
                    "volume" => (VOLUME, "/metadata/uid"),
                    "pod" => (POD, "/metadata/uid"),
                    _ => (POD, "/status/containerStatuses/0/containerID"),
                };
                *state
                    .rows
                    .get_mut(path)
                    .unwrap()
                    .pointer_mut(pointer)
                    .unwrap() = json!("replacement");
            }
        }
        let result = fixture
            .backend
            .inspect_native_runtime(
                &fixture.bound.runtime,
                &fixture.bound.request,
                &fixture.home,
            )
            .await;
        if matches!(change, "pod" | "container") {
            assert!(result.unwrap().is_none(), "requires bootstrap: {change}");
        } else {
            assert!(result.is_err(), "{change}");
        }
        assert_eq!(
            fixture.stream.requests.load(Ordering::SeqCst),
            0,
            "{change}"
        );
    }
}

#[tokio::test]
async fn native_status_lost_stream_and_partial_reply_do_not_replay() {
    for fault in [1, 2] {
        let fixture = fixture(false, true).await;
        fixture.stream.fault.store(fault, Ordering::SeqCst);
        assert!(fixture
            .adapter
            .inspect_runtime(&fixture.adoption)
            .await
            .is_err());
        assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            reads(&fixture),
            4,
            "no post-status proof after an ambiguous reply"
        );
    }
}

#[tokio::test]
async fn native_status_post_reply_home_change_and_retirement_still_fail() {
    let fixture = fixture(false, true).await;
    fixture.stream.replace_volume.store(true, Ordering::SeqCst);
    assert!(fixture
        .adapter
        .inspect_runtime(&fixture.adoption)
        .await
        .is_err());
    assert_eq!(reads(&fixture), 8);
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);

    let other = self::fixture(false, true).await;
    let held = other.stream.gate.acquire().await.unwrap();
    let adapter = other.adapter.clone();
    let adoption = other.adoption.clone();
    let pending = tokio::spawn(async move { adapter.inspect_runtime(&adoption).await });
    received(&other).await;
    other.transport.retire(&other.bound.runtime).await.unwrap();
    assert!(pending.await.unwrap().is_err());
    drop(held);
    assert_eq!(other.stream.requests.load(Ordering::SeqCst), 1);
}

#[path = "binding_inflight_tests.rs"]
mod inflight;

#[tokio::test]
async fn pending_parent_never_sends_authenticated_status_on_reused_connection() {
    let mut fixture = fixture(true, true).await;
    let (release, admission) = crate::InspectionAdmission::pending();
    let adapter = fixture.adapter.clone();
    let request = fixture.adoption.clone();
    let task =
        tokio::spawn(async move { adapter.inspect_runtime_after(&request, &admission).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while fixture.bound.fixture.state.lock().unwrap().calls.len() < 6 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 0);
    drop(release);
    assert!(task.await.unwrap().is_err());
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 0);
    // Cancellation deliberately drops NativeLease and its stream, even when no
    // bytes were sent. Observe that close, then supply a fresh fixture stream.
    tokio::time::timeout(std::time::Duration::from_secs(2), &mut fixture.task)
        .await
        .unwrap()
        .unwrap();
    let socket = fixture
        .transport
        .seed_native_status_connection(&fixture.bound.runtime)
        .await;
    fixture.task = tokio::spawn(serve(
        socket,
        fixture.stream.clone(),
        fixture.bound.fixture.state.clone(),
    ));
    assert!(fixture
        .adapter
        .inspect_runtime(&fixture.adoption)
        .await
        .unwrap()
        .is_some());
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
}

#[path = "backend_owned_status_tests.rs"]
mod owned;
use owned::write_owner_response;
