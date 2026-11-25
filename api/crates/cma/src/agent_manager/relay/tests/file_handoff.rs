//! Production relay and file handlers on two independent Mongo-backed managers.
use super::*;
use crate::store::{agent_ownership::MongoAgentOwnership, MongoSandboxStore};
use layer2_core::{
    resource_access::ResourceAccessService,
    sandbox::{
        agent_filesystem::AgentFileService,
        store::{SandboxStore, WorkspaceStore, WorktreeStore},
        Agent, AgentReadService, RuntimeBinding, Sandbox, SandboxResourceReadError,
        SandboxResourceReader, SandboxResourceReading,
    },
};
use mongodb::{Client, Database};
use runtime::{Files, FILE_BYTES, FILE_NAME, FILE_PATH, HANDLE, SANDBOX};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicUsize, Ordering::SeqCst},
    Mutex,
};

#[path = "file_handoff/home.rs"]
mod home;
#[path = "file_handoff/runtime.rs"]
mod runtime;
#[path = "file_handoff/traces.rs"]
mod traces;
#[path = "file_handoff/withdrawal.rs"]
mod withdrawal;
#[path = "file_handoff/writes.rs"]
mod writes;

struct NoResources;

#[async_trait::async_trait]
impl SandboxResourceReader for NoResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("the file routes do not sample runtime resources");
    }
}

async fn resources(admin: &Client, name: &str) -> Agent {
    let store = MongoSandboxStore::open(admin, name).await.unwrap();
    let (mut sandbox, agent) =
        crate::store::sandbox_maintenance::mongo_integration_tests::fixture(&store, SANDBOX).await;
    sandbox.lifecycle.home = Some(Box::new(home::native_home(&sandbox.id)));
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: HANDLE.into(),
        runtime_version: "fixture".into(),
        boot_id: Some("a".repeat(32)),
        provision_generation: Some(1),
        bound_at: sandbox.created_at.clone(),
    });
    sandbox.revision += 1;
    store
        .save_sandbox(sandbox.revision - 1, sandbox)
        .await
        .unwrap();
    store
        .create_workspace(crate::auth::test_resources::workspace(
            agent.workspace_id.as_str(),
            agent.sandbox_id.as_str(),
            agent.owner.as_str(),
        ))
        .await
        .unwrap();
    store
        .create_worktree(crate::auth::test_resources::worktree(
            agent.worktree_id.as_str(),
            agent.workspace_id.as_str(),
            agent.sandbox_id.as_str(),
            agent.id.as_str(),
            agent.owner.as_str(),
        ))
        .await
        .unwrap();
    agent
}

#[derive(Clone, Copy, Default)]
enum PeerReply {
    #[default]
    Production,
    Legacy,
    Busy,
    DependencyUnavailable,
    UncertainWrite,
}

#[derive(Default)]
struct PeerProbe {
    mode: Mutex<PeerReply>,
    requests: AtomicUsize,
    writes: AtomicUsize,
    pause_write_refusal: std::sync::atomic::AtomicBool,
    write_refused: tokio::sync::Notify,
    resume_write_reply: tokio::sync::Notify,
}

impl PeerProbe {
    async fn route(
        &self,
        request: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        if !request.headers().contains_key(PROOF) {
            return next.run(request).await;
        }
        self.requests.fetch_add(1, SeqCst);
        let mode = *self.mode.lock().unwrap();
        match mode {
            PeerReply::Production => {
                let response = next.run(request).await;
                if response
                    .headers()
                    .contains_key(file_write::NOT_ENTERED_HEADER)
                    && self.pause_write_refusal.swap(false, SeqCst)
                {
                    self.write_refused.notify_one();
                    self.resume_write_reply.notified().await;
                }
                response
            }
            PeerReply::Legacy => {
                let mut response = next.run(request).await;
                response.headers_mut().remove(file_read::HANDOFF_HEADER);
                response
            }
            PeerReply::Busy => {
                (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "7")]).into_response()
            }
            PeerReply::DependencyUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({"code": "file_unavailable"})),
            )
                .into_response(),
            PeerReply::UncertainWrite => {
                assert_eq!(request.method(), Method::POST);
                self.writes.fetch_add(1, SeqCst);
                // Even a peer that erroneously marks an ambiguous write must
                // not cause the ingress to replay that mutation.
                file_read::refusal(None, Refusal::AuthorityChanged)
            }
        }
    }
}

struct Replica {
    instance: Instance,
    runtime: Arc<Files>,
    probe: Arc<PeerProbe>,
    admission: crate::startup::Admission,
    relay_log: traces::RelayLog,
}

impl Replica {
    async fn bind(
        uri: &str,
        db: &Database,
        agent: &Agent,
        index: usize,
        write_contents: Option<Arc<Mutex<Vec<u8>>>>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let client = Client::with_uri_str(uri).await.unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, db.name()).await.unwrap());
        let ownership = Arc::new(MongoAgentOwnership::open(&client, db.name()).await.unwrap());
        let coordinator = OwnershipCoordinator::register(
            ownership,
            Member {
                instance_id: format!("file-fixture-{index}"),
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 16,
            },
        )
        .await
        .unwrap();
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-file-handoff-key"),
        )
        .unwrap();
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: agent.owner.to_string(),
            },
            None,
        );
        let access = Arc::new(ResourceAccessService::new(store.clone(), auth.clone()));
        let reads = AgentReadService::new(store, Arc::new(NoResources))
            .with_resource_access(access.clone());
        if write_contents.is_some() {
            relay.enable_file_write_handoff();
        }
        let runtime = Arc::new(Files::with_write_contents(write_contents));
        relay
            .install_public_access(PublicAccess::new(
                auth.clone(),
                reads,
                access.clone(),
                runtime.clone(),
            ))
            .unwrap();
        let router = layer1_public::rest::agent_filesystem_router(AgentFileService::new(
            access,
            runtime.clone(),
        ));
        let probe = Arc::new(PeerProbe::default());
        let observed_peer = probe.clone();
        let router = relay
            .authenticated_routes(router, auth)
            .layer(axum::middleware::from_fn(move |request, next| {
                let probe = observed_peer.clone();
                async move { probe.route(request, next).await }
            }));
        let admission = crate::startup::Admission::new();
        assert!(admission.open());
        let relay_log = traces::RelayLog::default();
        let router = relay_log
            .observe(admission.wrap(crate::with_request_context(router), Some(relay.clone())));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            instance: Instance {
                coordinator,
                relay,
                updates,
                address,
                server,
            },
            runtime,
            probe,
            admission,
            relay_log,
        }
    }
}

const ROUTES: [(&str, &str); 3] = [
    ("tree", "cma"),
    ("content", FILE_PATH),
    ("download", FILE_PATH),
];

async fn reads(peer: &Replica, id: &AgentId, phase: &str) -> Vec<(StatusCode, axum::body::Bytes)> {
    let mut results = Vec::with_capacity(ROUTES.len());
    let http = client();
    for (route, path) in ROUTES {
        let started = std::time::Instant::now();
        let response = http
            .get(format!(
                "{}/api/v1/agents/{id}/files/{route}",
                peer.instance.address
            ))
            .query(&[("path", path)])
            .send()
            .await
            .unwrap();
        assert!(!response.headers().contains_key(file_read::HANDOFF_HEADER));
        let status = response.status();
        let body = response.bytes().await.unwrap();
        let code = (!status.is_success())
            .then(|| serde_json::from_slice::<Value>(&body).unwrap()["code"].clone());
        println!(
            "file_handoff phase={phase} route={route} status={} code={code:?} elapsed_ms={}",
            status.as_u16(),
            started.elapsed().as_millis()
        );
        results.push((status, body));
    }
    results
}

fn assert_reads(results: &[(StatusCode, axum::body::Bytes)]) {
    for ((route, _), (status, _)) in ROUTES.iter().zip(results) {
        assert_eq!(*status, StatusCode::OK, "{route}");
    }
    let tree: Value = serde_json::from_slice(&results[0].1).unwrap();
    assert_eq!(tree["entries"][0]["name"], FILE_NAME);
    assert_eq!(tree["entries"][0]["sha256"], runtime::digest());
    let preview: Value = serde_json::from_slice(&results[1].1).unwrap();
    assert_eq!(preview["sha256"], runtime::digest());
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(preview["content_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, FILE_BYTES);
    assert_eq!(results[2].1.as_ref(), FILE_BYTES);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn ready_agent_file_reads_survive_owner_admission_closing() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_file_handoff_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    let agent = resources(&admin, &name).await;
    let replicas = [
        Replica::bind(&uri, &db, &agent, 0, None).await,
        Replica::bind(&uri, &db, &agent, 1, None).await,
    ];
    let membership = replicas[0]
        .instance
        .coordinator
        .store
        .membership()
        .await
        .unwrap();
    let selected = &candidates(&agent.id, &membership, None).unwrap()[0].instance_id;
    let owner_index = replicas
        .iter()
        .position(|replica| replica.instance.coordinator.instance_id == *selected)
        .unwrap();
    let owner = &replicas[owner_index];
    let peer = &replicas[1 - owner_index];
    let authority = owner
        .instance
        .coordinator
        .acquire(&agent.id)
        .await
        .unwrap()
        .unwrap();

    assert_reads(&reads(peer, &agent.id, "before").await);
    assert_eq!(owner.runtime.reads.load(SeqCst), 3);
    assert_eq!(peer.runtime.reads.load(SeqCst), 0);
    let authority = withdrawal::revoke_admitted_read(owner, peer, &agent.id, authority).await;
    owner.instance.coordinator.stop_admission();
    assert!(!owner.instance.coordinator.is_ready());
    assert_eq!(
        peer.instance
            .relay
            .destination(&agent.id)
            .await
            .unwrap()
            .token,
        authority.token
    );
    let during = reads(peer, &agent.id, "while_owner_drains").await;
    assert_reads(&during);
    peer.relay_log.assert_correlated(
        &owner.relay_log,
        "read",
        "peer_owner_handoff",
        StatusCode::OK,
    );
    assert_eq!(peer.runtime.reads.load(SeqCst), 4);
    assert_eq!(owner.runtime.reads.load(SeqCst), 4);
    assert_eq!(owner.probe.requests.load(SeqCst), 7);
    assert_eq!(peer.probe.requests.load(SeqCst), 0);
    assert_eq!(
        peer.instance
            .relay
            .destination(&agent.id)
            .await
            .unwrap()
            .token,
        authority.token
    );

    let make_request = |method, suffix: &str| {
        client().request(
            method,
            format!(
                "{}/api/v1/agents/{}/files{suffix}",
                peer.instance.address, agent.id
            ),
        )
    };
    let head = make_request(Method::HEAD, "/download")
        .query(&[("path", FILE_PATH)])
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert!(!head.headers().contains_key(file_read::HANDOFF_HEADER));
    assert!(head.bytes().await.unwrap().is_empty());
    assert_eq!(peer.runtime.reads.load(SeqCst), 5);

    *owner.probe.mode.lock().unwrap() = PeerReply::Legacy;
    for method in [Method::GET, Method::HEAD] {
        let legacy = make_request(method.clone(), "/download")
            .query(&[("path", FILE_PATH)])
            .send()
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(legacy.headers()["retry-after"], "1");
        if method == Method::GET {
            assert_eq!(
                legacy.json::<Value>().await.unwrap()["code"],
                "agent_owner_unavailable"
            );
        } else {
            assert!(legacy.bytes().await.unwrap().is_empty());
        }
    }
    *owner.probe.mode.lock().unwrap() = PeerReply::Busy;
    let busy = make_request(Method::GET, "/tree")
        .query(&[("path", "cma")])
        .send()
        .await
        .unwrap();
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(busy.headers()["retry-after"], "7");
    *owner.probe.mode.lock().unwrap() = PeerReply::DependencyUnavailable;
    let unrelated = make_request(Method::GET, "/download")
        .query(&[("path", FILE_PATH)])
        .send()
        .await
        .unwrap();
    assert_eq!(unrelated.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        unrelated.json::<Value>().await.unwrap()["code"],
        "file_unavailable"
    );
    *owner.probe.mode.lock().unwrap() = PeerReply::UncertainWrite;
    let write = make_request(Method::POST, "").send().await.unwrap();
    assert_eq!(write.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!write.headers().contains_key(file_read::HANDOFF_HEADER));
    assert_eq!(owner.probe.writes.load(SeqCst), 1);
    assert_eq!(owner.probe.requests.load(SeqCst), 13);
    assert_eq!(peer.runtime.reads.load(SeqCst), 5);

    *owner.probe.mode.lock().unwrap() = PeerReply::Production;
    peer.runtime.unavailable.store(true, SeqCst);
    let cold_binding = make_request(Method::GET, "/download")
        .query(&[("path", FILE_PATH)])
        .send()
        .await
        .unwrap();
    assert_eq!(cold_binding.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        cold_binding.json::<Value>().await.unwrap()["code"],
        "file_unavailable"
    );
    peer.runtime.unavailable.store(false, SeqCst);

    let store = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let original = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    peer.runtime.pause_next.store(true, SeqCst);
    let request = make_request(Method::GET, "/download")
        .query(&[("path", FILE_PATH)])
        .send();
    let change_binding = async {
        tokio::time::timeout(Duration::from_secs(2), peer.runtime.entered.notified())
            .await
            .unwrap();
        let mut replacement = original.clone();
        replacement.revision += 1;
        let binding = replacement.runtime.as_mut().unwrap();
        binding.runtime_id = "replacement-file-handoff-runtime".into();
        binding.boot_id = Some("b".repeat(32));
        store
            .save_sandbox(original.revision, replacement)
            .await
            .unwrap();
        peer.runtime.resume.notify_one();
    };
    let (response, ()) = tokio::join!(request, change_binding);
    let response = response.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_ne!(response.bytes().await.unwrap().as_ref(), FILE_BYTES);
    let mut restored_fixture = original;
    restored_fixture.revision += 2;
    store
        .save_sandbox(restored_fixture.revision - 1, restored_fixture)
        .await
        .unwrap();

    withdrawal::close_outer_admission(owner, peer, &agent.id, &authority).await;
    owner.instance.coordinator.begin_drain().await;
    owner
        .instance
        .coordinator
        .handoff(&authority, authority.revoke_transports())
        .await
        .unwrap();
    let successor = peer
        .instance
        .coordinator
        .acquire(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > authority.token.epoch);
    assert_reads(&reads(peer, &agent.id, "after").await);
    drop(replicas);
    db.drop().await.unwrap();
}
