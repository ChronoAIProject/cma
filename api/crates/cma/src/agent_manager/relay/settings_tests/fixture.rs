use super::runtime::Runtime;
use super::*;
use crate::auth::test_resources as resources;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::*;
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const OWNER: &str = "settings-owner";
pub(super) const AGENT: &str = "agt-settings";

pub(super) struct Replica {
    pub store: Arc<MongoSandboxStore>,
    pub access: Arc<ResourceAccessService>,
    pub coordinator: Option<Arc<OwnershipCoordinator>>,
    pub relay: Option<Arc<Relay>>,
    pub address: String,
    pub drop_response: Arc<AtomicBool>,
    pub scopes: Arc<Mutex<Vec<Option<OwnershipToken>>>>,
    pub runtime: Arc<Runtime>,
    server: JoinHandle<()>,
    renewal: JoinHandle<()>,
}

impl Drop for Replica {
    fn drop(&mut self) {
        if let Some(coordinator) = &self.coordinator {
            coordinator.stop_admission();
        }
        self.server.abort();
        self.renewal.abort();
    }
}

impl Replica {
    pub async fn open(name: &str, coordinated: bool) -> Self {
        Self::open_as(name, coordinated, OWNER).await
    }

    pub async fn open_as(name: &str, coordinated: bool, subject: &str) -> Self {
        let client =
            mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                .await
                .unwrap();
        let instance = uuid::Uuid::new_v4().simple().to_string();
        let store = MongoSandboxStore::open(&client, name).await.unwrap();
        let store = Arc::new(if coordinated {
            store
                .enable_agent_ownership(instance.clone())
                .await
                .unwrap()
        } else {
            store
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: subject.into(),
            },
            None,
        );
        let access = Arc::new(ResourceAccessService::new(store.clone(), auth.clone()));
        let runtime = Arc::new(Runtime::new(store.clone()));
        if !coordinated {
            runtime.attach();
        }
        let reads = AgentReadService::new(store.clone(), runtime.clone())
            .with_resource_access(access.clone());
        let agents = AgentService::new(
            store.clone(),
            Arc::new(InMemoryProfileStore::default()),
            runtime.clone(),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        );
        let operations = AgentOperationService::new(store.clone(), runtime.clone());
        let records = Arc::new(
            crate::store::operation::MongoOperationStore::open_for_test(&client, name)
                .await
                .unwrap(),
        );
        let scopes = Arc::new(Mutex::new(Vec::new()));
        let observed = scopes.clone();
        let public = layer1_public::rest::agent_router(layer1_public::rest::AgentRoutesState::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://settings.example.test")
                .unwrap(),
            agents,
            reads.clone(),
            operations,
            records,
        ))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = observed.clone();
                async move {
                    if request.method() == Method::PATCH
                        && request.uri().path().ends_with("/settings")
                    {
                        observed
                            .lock()
                            .unwrap()
                            .push(layer2_core::agent_ownership::execution_owner());
                    }
                    next.run(request).await
                }
            },
        ));
        let mut coordinator = None;
        let mut relay = None;
        let app = if coordinated {
            let owner = OwnershipCoordinator::register(
                store.agent_ownership.clone().unwrap(),
                Member {
                    instance_id: instance,
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
            let route = Relay::new(
                owner.clone(),
                &updates,
                ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"settings-relay-fixture"),
            )
            .unwrap();
            let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
            route
                .install_public_access(PublicAccess::new(
                    auth.clone(),
                    reads,
                    access.clone(),
                    Arc::new(adapter),
                ))
                .unwrap();
            route.enable_settings_owner_routing();
            let app = route.authenticated_routes(public, auth);
            coordinator = Some(owner);
            relay = Some(route);
            app
        } else {
            public.layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::attach_subject,
            ))
        };
        let drop_response = Arc::new(AtomicBool::new(false));
        let drop_next = drop_response.clone();
        let app = app.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let drop_next = drop_next.clone();
                async move {
                    let response = next.run(request).await;
                    if response.status().is_success() && drop_next.swap(false, Ordering::SeqCst) {
                        let (parts, _) = response.into_parts();
                        let body =
                            axum::body::Body::from_stream(futures_util::stream::once(async {
                                Err::<axum::body::Bytes, _>(std::io::Error::other(
                                    "synthetic lost settings response",
                                ))
                            }));
                        axum::response::Response::from_parts(parts, body)
                    } else {
                        response
                    }
                }
            },
        ));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let renew = coordinator.clone();
        let renewal = tokio::spawn(async move {
            if let Some(owner) = renew {
                let mut ticks = tokio::time::interval(Duration::from_millis(
                    layer2_core::agent_ownership::HEARTBEAT_INTERVAL_MS,
                ));
                loop {
                    ticks.tick().await;
                    if owner.is_draining() {
                        break;
                    }
                    let result = owner.maintain().await;
                    if !owner.is_draining() {
                        result.unwrap();
                    }
                }
            }
        });
        Self {
            store,
            access,
            coordinator,
            relay,
            address,
            drop_response,
            scopes,
            runtime,
            server,
            renewal,
        }
    }

    pub async fn request(
        &self,
        method: Method,
        key: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = client().request(
            method,
            format!("{}/api/v1/agents/{AGENT}/settings", self.address),
        );
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.json().await.unwrap())
    }

    pub async fn close(mut self) {
        if let Some(coordinator) = &self.coordinator {
            for authority in coordinator.begin_drain().await {
                coordinator
                    .handoff(&authority, async { Ok(()) })
                    .await
                    .unwrap();
            }
        }
        self.server.abort();
        self.renewal.abort();
        for task in [&mut self.server, &mut self.renewal] {
            if let Err(error) = task.await {
                assert!(error.is_cancelled(), "fixture task failed: {error}");
            }
        }
    }
}

pub(super) async fn seed(store: &MongoSandboxStore) {
    let mut sandbox = resources::sandbox("sbx-settings", OWNER);
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "runtime-settings".into(),
        runtime_version: "fixture-image".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: sandbox.created_at.clone(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 1;
    store.create_sandbox(sandbox).await.unwrap();
    store
        .create_workspace(resources::workspace("wsp-settings", "sbx-settings", OWNER))
        .await
        .unwrap();
    store
        .create_worktree(resources::worktree(
            "wkt-settings",
            "wsp-settings",
            "sbx-settings",
            AGENT,
            OWNER,
        ))
        .await
        .unwrap();
    let mut agent = resources::agent(
        AGENT,
        "wkt-settings",
        "wsp-settings",
        "sbx-settings",
        OWNER,
        AgentStatus::Ready,
    );
    agent.profile.content.model = Some("gpt-6".into());
    agent.profile.content.thinking_effort = layer2_core::profiles::ThinkingEffort::High;
    agent.profile.content.permission_level = layer2_core::profiles::PermissionLevel::Autonomous;
    agent.conversation.settings = Some(AgentSettings {
        model: "gpt-6".into(),
        reasoning_effort: layer2_core::profiles::ThinkingEffort::High,
        permission_mode: layer2_core::profiles::PermissionLevel::Autonomous,
        system_prompt: Some("Retained instructions".into()),
    });
    agent.ready_at = Some(agent.created_at.clone());
    agent.conversation.conversation_id = Some("conversation-settings".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 0;
    store.create_agent(agent).await.unwrap();
}

pub(super) async fn acquire(replicas: &[Replica]) -> (usize, Arc<Authority>) {
    let coordinator = replicas[0].coordinator.as_ref().unwrap();
    let id = AgentId(AGENT.into());
    let membership = coordinator.store.membership().await.unwrap();
    let preferred = candidates(&id, &membership, None).unwrap()[0]
        .instance_id
        .clone();
    let owner = replicas
        .iter()
        .position(|replica| replica.coordinator.as_ref().unwrap().instance_id == preferred)
        .unwrap();
    let authority = replicas[owner]
        .coordinator
        .as_ref()
        .unwrap()
        .acquire(&id)
        .await
        .unwrap()
        .unwrap();
    replicas[owner].runtime.attach();
    (owner, authority)
}
