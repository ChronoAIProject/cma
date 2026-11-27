use super::*;
use crate::{
    agent_sandbox::{AgentSandboxClient, NativeHomeVolumes},
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::HostedDeployment,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[path = "backend_inspection_admission_tests.rs"]
mod inspection_admission;

#[path = "backend_policy_read_tests.rs"]
mod policy_reads;

#[path = "backend_file_read_tests.rs"]
mod file_reads;

#[path = "backend_post_status_order_tests.rs"]
mod post_status_order;

#[path = "backend_native_status_tests.rs"]
mod native_status;

#[path = "binding_race_tests.rs"]
mod binding_races;

#[path = "backend_adoption_routing_tests.rs"]
mod adoption_routing;

struct StatusTransport {
    state: Arc<Mutex<State>>,
    client: AgentSandboxClient,
    calls: AtomicUsize,
    replace_volume: AtomicBool,
    after_status: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}
#[async_trait::async_trait]
impl HostedControlTransport for StatusTransport {
    async fn bind_runtime(&self, _: &RuntimeHandle, sandbox: &str) -> Result<()> {
        assert_eq!(sandbox, "sbx_one");
        Ok(())
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        let RequestFrame::RuntimeStatus(request) = request else {
            panic!("status only")
        };
        // Model the unchanged fresh dispatch boundary separately from the proof.
        self.client.pod_binding(handle).await?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.replace_volume.load(Ordering::SeqCst) {
            self.state.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] =
                json!("replaced-after-status");
        }
        if let Some(after_status) = self.after_status.lock().unwrap().take() {
            after_status();
        }
        Ok(ResponseFrame::RuntimeStatus(RuntimeStatus {
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
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("status only")
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        Ok(())
    }
}
struct InspectionFixture {
    bound: Bound,
    adapter: CodexAdapter,
    backend: Arc<AgentSandboxBackend>,
    transport: Arc<StatusTransport>,
    adoption: RuntimeAdoptionRequest,
    home: HomeVolumeAttachment,
}
async fn inspection_fixture(explicit_policy: bool) -> InspectionFixture {
    inspection_fixture_with_home_authority(explicit_policy, "fixture").await
}
async fn inspection_fixture_with_home_authority(
    explicit_policy: bool,
    authority: &str,
) -> InspectionFixture {
    let bound = bound(explicit_policy).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    let transport = Arc::new(StatusTransport {
        state: bound.fixture.state.clone(),
        client: client(&bound.fixture, false),
        calls: AtomicUsize::new(0),
        replace_volume: AtomicBool::new(false),
        after_status: Mutex::new(None),
    });
    let backend = Arc::new(AgentSandboxBackend::new(
        client(&bound.fixture, false),
        transport.clone(),
    ));
    assert_eq!(
        backend.provision(bound.request.clone()).await.unwrap(),
        bound.runtime
    );
    let config = super::super::super::tests::spec();
    let adapter = CodexAdapter::new(
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
        crate::RoutedHomeVolumes::new(
            None,
            Some((
                Arc::new(
                    NativeHomeVolumes::new(
                        bound.fixture.kubernetes.clone(),
                        bound.fixture.client.config.clone(),
                        authority,
                    )
                    .unwrap(),
                ),
                home.request.policy.clone(),
            )),
        )
        .unwrap(),
    ));
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
    bound.fixture.state.lock().unwrap().calls.clear();
    InspectionFixture {
        bound,
        adapter,
        backend,
        transport,
        adoption,
        home,
    }
}
fn read_count(fixture: &InspectionFixture) -> usize {
    let state = fixture.bound.fixture.state.lock().unwrap();
    assert!(state
        .calls
        .iter()
        .all(|(method, _)| method.starts_with("GET ")));
    state.calls.len()
}

#[tokio::test]
async fn native_proof_context_with_routed_home_accepts_only_configured_authority() {
    let fixture = inspection_fixture(false).await;
    let inspected = fixture
        .adapter
        .inspect_runtime(&fixture.adoption)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inspected.observation.handle, fixture.bound.runtime);
    assert!(inspected.status.unwrap().credential_ready);
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(read_count(&fixture), 13);

    let foreign = inspection_fixture_with_home_authority(false, "different-cluster").await;
    assert!(matches!(
        foreign.adapter.inspect_runtime(&foreign.adoption).await,
        Err(AdapterError::Rejected(reason)) if reason == "native home belongs to another authority"
    ));
    assert_eq!(foreign.transport.calls.load(Ordering::SeqCst), 0);
    assert_eq!(read_count(&foreign), 0);
}

#[tokio::test]
async fn native_proof_context_shares_dispatch_reads_but_revalidates_after_each_reply() {
    let fixture = inspection_fixture(false).await;
    fixture
        .adapter
        .adopt_runtime(&fixture.adoption)
        .await
        .unwrap()
        .unwrap();
    fixture
        .adapter
        .runtime_status(&fixture.bound.runtime)
        .await
        .unwrap();
    // Generic adoption must observe persisted upgrade proof even when an explicit
    // rollback returns to the original pool image. The combined path shares that read.
    assert_eq!(
        read_count(&fixture),
        30,
        "adoption and status each repeat exact authority after their response"
    );
    fixture.bound.fixture.state.lock().unwrap().calls.clear();
    let inspected = fixture
        .adapter
        .inspect_runtime(&fixture.adoption)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inspected.observation.handle, fixture.bound.runtime);
    assert!(inspected.status.unwrap().credential_ready);
    assert_eq!(
        read_count(&fixture),
        13,
        "4 authority + 2 dispatch + 4 post-status authority + 3 home reads"
    );
    fixture.bound.fixture.state.lock().unwrap().calls.clear();
    fixture
        .adapter
        .inspect_runtime(&fixture.adoption)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        read_count(&fixture),
        13,
        "next operation must acquire all fresh evidence again"
    );
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        fixture
            .bound
            .fixture
            .state
            .lock()
            .unwrap()
            .assignments
            .len(),
        2
    );
}

#[tokio::test]
async fn native_proof_context_rejects_changed_authority_before_status() {
    for change in [
        "claim_uid",
        "assignment",
        "claim_origin",
        "member_uid",
        "member_spec",
        "member_origin",
        "member_owner",
        "volume_uid",
        "volume_spec",
        "pod_uid",
        "container",
        "request",
        "policy",
    ] {
        let mut fixture = inspection_fixture(change == "policy").await;
        match change {
            "claim_uid" => {
                fixture
                    .bound
                    .fixture
                    .edit("sandboxclaims", &fixture.bound.claim.name, |v| {
                        v["metadata"]["uid"] = json!("replacement")
                    })
            }
            "assignment" => {
                fixture
                    .bound
                    .fixture
                    .edit("sandboxclaims", &fixture.bound.claim.name, |v| {
                        v["status"]["sandbox"]["name"] = json!("other-member")
                    })
            }
            "claim_origin" => {
                fixture
                    .bound
                    .fixture
                    .edit("sandboxclaims", &fixture.bound.claim.name, |v| {
                        v["metadata"]["annotations"][crate::agent_sandbox::NATIVE_HOME_ANNOTATION] =
                            json!("{}")
                    })
            }
            "policy" => fixture.bound.fixture.edit(
                "sandboxtemplates",
                &fixture.bound.claim.request.pool.template_name,
                |v| v["spec"]["networkPolicy"]["egress"] = json!([{}]),
            ),
            "request" => {
                fixture
                    .adoption
                    .spec
                    .environment
                    .insert("CHANGED".into(), "value".into());
            }
            _ => {
                let mut state = fixture.bound.fixture.state.lock().unwrap();
                let (path, pointer, value) = match change {
                    "member_uid" => (MEMBER, "/metadata/uid", json!("replacement")),
                    "member_spec" => (
                        MEMBER,
                        "/spec/podTemplate/spec/containers/0/image",
                        json!("replacement"),
                    ),
                    "member_origin" => (
                        MEMBER,
                        "/metadata/annotations/cma~1native-home-origin",
                        json!("{}"),
                    ),
                    "member_owner" => (
                        MEMBER,
                        "/metadata/ownerReferences/0/uid",
                        json!("replacement"),
                    ),
                    "volume_uid" => (VOLUME, "/metadata/uid", json!("replacement")),
                    "volume_spec" => (VOLUME, "/spec/storageClassName", json!("replacement")),
                    "pod_uid" => (POD, "/metadata/uid", json!("replacement")),
                    "container" => (
                        POD,
                        "/status/containerStatuses/0/containerID",
                        json!("containerd://replacement"),
                    ),
                    _ => unreachable!(),
                };
                *state
                    .rows
                    .get_mut(path)
                    .unwrap()
                    .pointer_mut(pointer)
                    .unwrap() = value;
            }
        }
        // A replaced Pod/container is eligible only for full bootstrap, never this proof path.
        if matches!(change, "pod_uid" | "container") {
            assert!(
                fixture
                    .backend
                    .inspect_native_runtime(
                        &fixture.bound.runtime,
                        &fixture.bound.request,
                        &fixture.home
                    )
                    .await
                    .unwrap()
                    .is_none(),
                "{change}"
            );
        } else {
            assert!(
                fixture
                    .adapter
                    .inspect_runtime(&fixture.adoption)
                    .await
                    .is_err(),
                "{change}"
            );
        }
        assert_eq!(
            fixture.transport.calls.load(Ordering::SeqCst),
            0,
            "{change}"
        );
    }
}

#[tokio::test]
async fn native_proof_context_retains_post_status_home_fence_and_missing_ack_fallback() {
    let fixture = inspection_fixture(false).await;
    let restarted = AgentSandboxBackend::new(
        client(&fixture.bound.fixture, false),
        fixture.transport.clone(),
    );
    assert!(restarted
        .inspect_native_runtime(
            &fixture.bound.runtime,
            &fixture.bound.request,
            &fixture.home
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 0);
    fixture
        .transport
        .replace_volume
        .store(true, Ordering::SeqCst);
    assert!(fixture
        .adapter
        .inspect_runtime(&fixture.adoption)
        .await
        .is_err());
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_proof_reads_all_four_known_resources_in_one_round() {
    let fixture = inspection_fixture(false).await;
    let rows = Arc::new(fixture.bound.fixture.state.lock().unwrap().rows.clone());
    let gate = Arc::new(tokio::sync::Barrier::new(4));
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = calls.clone();
    let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        assert_eq!(request.method(), http::Method::GET);
        let path = request.uri().path().to_owned();
        let body = if path == "/api/v1/namespaces/cma-test/pods" {
            json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},"items":[rows[POD].clone()]})
        } else {
            rows.get(&path)
                .expect("exact persisted resource name")
                .clone()
        };
        let gate = gate.clone();
        recorded.fetch_add(1, Ordering::SeqCst);
        async move {
            // No response is released until all four reads have begun.
            gate.wait().await;
            Ok::<_, std::convert::Infallible>(
                http::Response::builder()
                    .status(200)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
        }
    });
    let client = AgentSandboxClient::new(
        kube::Client::new(service, "cma-test"),
        fixture.bound.fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    let proof = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.native_runtime_proof(
            &fixture.bound.runtime,
            &fixture.bound.request,
            &fixture.home,
        ),
    )
    .await
    .expect("sequential reads cannot pass the four-resource barrier")
    .unwrap();
    assert!(proof
        .matches(&fixture.bound.request, &fixture.home)
        .unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}
