use super::*;
use crate::auth::test_resources as resources;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::*;
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const OWNER: &str = "preferences-owner";
pub(super) const AGENT: &str = "agt-preferences";

pub(super) struct Runtime {
    pub inputs: Arc<Mutex<Vec<layer3_adapter::ConversationInput>>>,
    pub supported: Arc<AtomicBool>,
}

struct Instructions(Arc<AtomicBool>);
#[async_trait::async_trait]
impl layer3_adapter::ConversationSettingsSupport for Instructions {
    async fn system_prompt_supported(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let inputs = self.inputs.clone();
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: AgentRuntimeInputSender::spawn(4, move |input| {
                inputs.lock().unwrap().push(input);
                async { Ok(()) }
            })
            .with_settings_support(Arc::new(Instructions(self.supported.clone()))),
        })
    }

    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl SandboxResourceReader for Runtime {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("preferences cannot read runtime resource usage")
    }
}

pub(super) struct Replica {
    pub store: Arc<MongoSandboxStore>,
    pub coordinator: Option<Arc<OwnershipCoordinator>>,
    pub relay: Option<Arc<Relay>>,
    pub address: String,
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
                subject: OWNER.into(),
            },
            None,
        );
        let access = Arc::new(ResourceAccessService::new(store.clone(), auth.clone()));
        let runtime = Arc::new(Runtime {
            inputs: Default::default(),
            supported: Arc::new(AtomicBool::new(true)),
        });
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
            layer1_public::agent_page::AgentPageOrigin::new("https://preferences.example.test")
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
                        && request.uri().path().ends_with("/preferences")
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
                ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"preferences-relay-fixture"),
            )
            .unwrap();
            let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
            route
                .install_public_access(PublicAccess::new(
                    auth.clone(),
                    reads,
                    access,
                    Arc::new(adapter),
                ))
                .unwrap();
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
            coordinator,
            relay,
            address,
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
            format!("{}/api/v1/agents/{AGENT}/preferences", self.address),
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
    let mut sandbox = resources::sandbox("sbx-preferences", OWNER);
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "runtime-preferences".into(),
        runtime_version: "fixture-image".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: sandbox.created_at.clone(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 1;
    store.create_sandbox(sandbox).await.unwrap();
    store
        .create_workspace(resources::workspace(
            "wsp-preferences",
            "sbx-preferences",
            OWNER,
        ))
        .await
        .unwrap();
    store
        .create_worktree(resources::worktree(
            "wkt-preferences",
            "wsp-preferences",
            "sbx-preferences",
            AGENT,
            OWNER,
        ))
        .await
        .unwrap();
    let mut agent = resources::agent(
        AGENT,
        "wkt-preferences",
        "wsp-preferences",
        "sbx-preferences",
        OWNER,
        AgentStatus::Ready,
    );
    agent.ready_at = Some(agent.created_at.clone());
    agent.conversation.conversation_id = Some("conversation-preferences".into());
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
    (owner, authority)
}

pub(super) async fn title_count(db: &mongodb::Database) -> u64 {
    db.collection::<mongodb::bson::Document>("agent_log")
        .count_documents(mongodb::bson::doc! {
            "agent_id": AGENT, "record.kind.type": "title_set"
        })
        .await
        .unwrap()
}
