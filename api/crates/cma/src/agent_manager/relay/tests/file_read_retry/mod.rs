//! The steady-owner #2779 reproduction through the public file router.
use super::*;
use crate::store::{agent_ownership::MongoAgentOwnership, MongoSandboxStore};
use l3::hosted::HostedControlTransport;
use l3::wire::RequestFrame;
use l3::SandboxAdapter;
use layer2_core::{
    resource_access::ResourceAccessService,
    sandbox::{
        agent_filesystem::AgentFileService,
        store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore},
        Agent, AgentReadService, AgentStatus, RuntimeBinding, Sandbox, SandboxResourceReadError,
        SandboxResourceReader, SandboxResourceReading,
    },
};
use layer3_adapter as l3;
use mongodb::{Client, Database};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{sync::atomic::Ordering::SeqCst, time::Instant};

mod backend;
mod conditional_edits;
mod home;
mod runtime;
use backend::FileBackend;
use runtime::{Challenge, Runtime, HANDLE};

const FILE_NAME: &str = "read.txt";
const FILE_PATH: &str = "cma/read.txt";
const FILE_BYTES: &[u8] = b"fixture for owned file reads\n";
const SANDBOX: &str = "sbx_file_retry";

fn digest() -> String {
    format!("{:x}", Sha256::digest(FILE_BYTES))
}

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

async fn resources(admin: &Client, name: &str) -> (Agent, l3::HomeVolumeAttachment) {
    let store = MongoSandboxStore::open(admin, name).await.unwrap();
    let (mut sandbox, agent) =
        crate::store::sandbox_maintenance::mongo_integration_tests::fixture(&store, SANDBOX).await;
    let home = home::native_home(&sandbox.id);
    let attachment = home.attachment().unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
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
    (agent, attachment)
}

struct Steady {
    db: Database,
    agent: Agent,
    owner: Instance,
    store: Arc<MongoSandboxStore>,
    http: reqwest::Client,
    maintenance: JoinHandle<()>,
}

impl Steady {
    async fn bind(uri: &str, runtime: &Runtime, conditional_write_retry: bool) -> Self {
        let admin = Client::with_uri_str(uri).await.unwrap();
        let name = format!("cma_file_steady_{}", uuid::Uuid::new_v4().simple());
        let db = admin.database(&name);
        let (agent, attachment) = resources(&admin, &name).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let client = Client::with_uri_str(uri).await.unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let ownership = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
        let coordinator = OwnershipCoordinator::prepare_with_options(
            ownership,
            Member {
                instance_id: "steady-owner".into(),
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 16,
            },
            false,
            true,
        );
        coordinator.activate().await.unwrap();
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-steady-owner-key"),
        )
        .unwrap();
        if conditional_write_retry {
            relay.enable_file_write_handoff();
        }
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: agent.owner.to_string(),
            },
            None,
        );
        let access = Arc::new(ResourceAccessService::new(store.clone(), auth.clone()));
        let files = Arc::new(
            l3::codex::CodexAdapter::with_backend(
                FileBackend(runtime.transport.clone()),
                l3::codex::CodexSandboxSpec::default(),
            )
            .with_home_volumes(Arc::new(backend::Home(attachment.clone()))),
        );
        let handle = files
            .provision(&l3::SandboxSpec {
                sandbox_id: SANDBOX.into(),
                home_volume: Some(Box::new(attachment)),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(handle.as_str(), HANDLE);
        let reads = AgentReadService::new(store.clone(), Arc::new(NoResources))
            .with_resource_access(access.clone());
        relay
            .install_public_access(PublicAccess::new(
                auth.clone(),
                reads,
                access.clone(),
                files.clone(),
            ))
            .unwrap();
        let observed = runtime.counts.clone();
        let router =
            layer1_public::rest::agent_filesystem_router(AgentFileService::new(access, files))
                .layer(axum::middleware::from_fn(
                    move |request: axum::extract::Request, next: axum::middleware::Next| {
                        let observed = observed.clone();
                        async move {
                            if request.method() == Method::POST {
                                observed.edit_admissions.fetch_add(1, SeqCst);
                            }
                            next.run(request).await
                        }
                    },
                ));
        let router = relay.authenticated_routes(router, auth);
        let router = crate::with_request_context(router);
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let keeper = coordinator.clone();
        let maintenance = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                assert!(keeper.maintain().await.unwrap().is_empty());
            }
        });
        Self {
            db,
            agent,
            owner: Instance {
                coordinator,
                relay,
                updates,
                address,
                server,
            },
            store,
            http: client_http(),
            maintenance,
        }
    }

    async fn read(&self, method: Method, route: &str) -> (u16, String, f64) {
        let path = if route == "tree" { "cma" } else { FILE_PATH };
        let started = Instant::now();
        let head = method == Method::HEAD;
        let response = self
            .http
            .request(
                method,
                format!(
                    "{}/api/v1/agents/{}/files/{route}",
                    self.owner.address, self.agent.id
                ),
            )
            .query(&[("path", path)])
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body = response.bytes().await.unwrap();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let code = if head {
            assert!(body.is_empty());
            String::new()
        } else if status == 200 {
            if route == "download" {
                assert_eq!(body.as_ref(), FILE_BYTES);
            } else {
                let value: Value = serde_json::from_slice(&body).unwrap();
                if route == "tree" {
                    assert_eq!(value["entries"][0]["name"], FILE_NAME);
                    assert_eq!(value["entries"][0]["sha256"], digest());
                } else {
                    use base64::Engine as _;
                    assert_eq!(value["sha256"], digest());
                    assert_eq!(
                        base64::engine::general_purpose::STANDARD
                            .decode(value["content_base64"].as_str().unwrap())
                            .unwrap(),
                        FILE_BYTES,
                    );
                }
            }
            String::new()
        } else {
            serde_json::from_slice::<Value>(&body).unwrap()["code"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        println!("FILE READ route={route} status={status} code={code} ms={ms:.3}");
        (status, code, ms)
    }

    async fn authority(&self) -> Arc<Authority> {
        self.owner
            .coordinator
            .acquire(&self.agent.id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn verify_owner(&self, owner: &Authority) {
        let coordinator = &self.owner.coordinator;
        assert!(coordinator.is_ready() && !coordinator.is_draining());
        assert!(!self.maintenance.is_finished());
        assert_eq!(
            coordinator
                .store
                .owner(&self.agent.id)
                .await
                .unwrap()
                .unwrap()
                .token,
            owner.token,
        );
        let cohort = coordinator.store.membership().await.unwrap();
        assert_eq!(cohort.members.len(), 1);
        assert!(cohort.members[0].is_live(cohort.now_ms));
        assert_eq!(cohort.members[0].instance_id, owner.token.instance_id);
        assert_eq!(
            self.store
                .get_agent(&self.agent.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AgentStatus::Ready,
        );
    }

    async fn cleanup(&self, authority: &Arc<Authority>) {
        self.owner
            .coordinator
            .handoff(authority, authority.revoke_transports())
            .await
            .unwrap();
        let next = self.authority().await;
        assert_eq!(next.token.instance_id, authority.token.instance_id);
        assert!(next.token.epoch > authority.token.epoch);
    }
}

fn client_http() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(9))
        .build()
        .unwrap()
}

impl Steady {
    async fn close(self) {
        self.maintenance.abort();
        self.owner.server.abort();
        self.db.drop().await.unwrap();
    }
}

fn script(runtime: &Runtime, challenges: impl IntoIterator<Item = Challenge>) {
    *runtime.counts.challenges.lock().unwrap() = challenges.into_iter().collect();
}

fn dispatches(runtime: &Runtime) -> (usize, usize, usize) {
    let counts = &runtime.counts;
    (
        counts.challenge.load(SeqCst),
        counts.confirm.load(SeqCst),
        counts.files.load(SeqCst),
    )
}

fn since(runtime: &Runtime, before: (usize, usize, usize)) -> (usize, usize, usize) {
    let after = dispatches(runtime);
    (after.0 - before.0, after.1 - before.1, after.2 - before.2)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn owner_file_reads_renegotiate_once_and_keep_receiver_fences() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let runtime = Runtime::start().await;
    let steady = Steady::bind(&uri, &runtime, false).await;
    let _ = steady.authority().await;

    for (method, route, first) in [
        (Method::GET, "tree", Challenge::Refuse),
        (Method::GET, "content", Challenge::Stall),
        (Method::GET, "download", Challenge::LoseReply),
        (Method::HEAD, "download", Challenge::Refuse),
    ] {
        let before = dispatches(&runtime);
        let issued = runtime.counts.issued.lock().unwrap().len();
        let confirmed = runtime.counts.confirmed.lock().unwrap().len();
        script(&runtime, [first]);
        let result = steady.read(method, route).await;
        assert_eq!(result.0, 200, "{result:?}");
        assert!(result.2 < 8_000.0, "{result:?}");
        assert_eq!(since(&runtime, before), (3, 2, 1));
        if matches!(first, Challenge::LoseReply) {
            let issued_nonces = runtime.counts.issued.lock().unwrap();
            let confirmed_nonces = runtime.counts.confirmed.lock().unwrap();
            // The lost 200 body never supplies a nonce to Confirm.
            assert_eq!(issued_nonces.len() - issued, 3);
            assert_eq!(&confirmed_nonces[confirmed..], &issued_nonces[issued + 1..]);
        }
        let authority = steady.authority().await;
        steady.verify_owner(&authority).await;
    }
    for (scenario, challenges, confirmation_lost, expected) in [
        (
            "persistent refusal",
            vec![Challenge::Refuse, Challenge::Refuse],
            false,
            (2, 0, 0),
        ),
        (
            "persistent stall",
            vec![Challenge::Stall, Challenge::Stall],
            false,
            (2, 0, 0),
        ),
        (
            "status consumed retry",
            vec![Challenge::Refuse, Challenge::Accept, Challenge::Refuse],
            false,
            (3, 1, 0),
        ),
        ("stale receiver", vec![Challenge::Stale], false, (1, 0, 0)),
        ("ambiguous confirmation", vec![], true, (1, 1, 0)),
        (
            "replacement Pod",
            vec![Challenge::ReplacePod],
            false,
            (1, 0, 0),
        ),
    ] {
        let authority = steady.authority().await;
        let before = dispatches(&runtime);
        script(&runtime, challenges);
        runtime
            .counts
            .lose_confirmation
            .store(confirmation_lost, SeqCst);
        let result = steady.read(Method::GET, "content").await;
        assert_eq!(
            (result.0, result.1.as_str()),
            (503, "agent_owner_unavailable"),
            "{scenario}"
        );
        assert!(result.2 < 8_000.0, "{scenario}: {result:?}");
        assert_eq!(since(&runtime, before), expected, "{scenario}");
        assert!(authority.receiver_cancelled(), "{scenario}");
        steady.verify_owner(&authority).await;
        runtime.counts.restarted.store(false, SeqCst);
        steady.cleanup(&authority).await;
    }

    let authority = steady.authority().await;
    let before = dispatches(&runtime);
    script(&runtime, [Challenge::Refuse]);
    let handle = l3::RuntimeHandle::new(HANDLE);
    let result = authority
        .run(runtime.transport.exchange(&handle, RequestFrame::Resources))
        .await;
    assert!(result.is_err());
    assert!(authority.receiver_cancelled());
    assert_eq!(since(&runtime, before), (1, 0, 0));
    steady.cleanup(&authority).await;

    let authority = steady.authority().await;
    script(&runtime, [Challenge::Stall]);
    let before = dispatches(&runtime);
    let respond = steady.read(Method::GET, "tree");
    let revoke_during_challenge = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.counts.challenge.load(SeqCst) == before.0 {
                runtime.counts.entered.notified().await;
            }
        })
        .await
        .unwrap();
        authority.revoke();
    };
    let (result, ()) = tokio::join!(respond, revoke_during_challenge);
    assert_eq!(
        (result.0, result.1.as_str()),
        (503, "agent_owner_unavailable")
    );
    assert_eq!(since(&runtime, before), (1, 0, 0));
    steady.verify_owner(&authority).await;
    steady.close().await;
}
