use super::*;

pub(super) async fn active_siblings() -> (Fixture, Sandbox, Vec<Agent>, SystemTime) {
    let mut f = Fixture::new().await;
    let created = f.harness.create().await;
    let mut now = start_time(&created);
    let active = f
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    f.harness.adapter = f.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        f.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let mut children = Vec::new();
    for number in 1..=2 {
        let mut agent =
            lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, number).await;
        let snapshot = f.backend.snapshot().await;
        let launch = snapshot
            .agents
            .get(&layer3_adapter::AgentSlot::new(agent.id.to_string()))
            .unwrap();
        agent.work.receipts.push(AgentWorkReceipt {
            step: AgentWorkStep::OpenConversation,
            operation_id: launch.operation_id.clone(),
            request_sha256: launch.request_sha256.clone(),
            completed_at: rfc3339_at(now),
        });
        let revision = agent.revision;
        agent.revision += 1;
        f.harness
            .store
            .save_agent(revision, agent.clone())
            .await
            .unwrap();
        let mut workspace = lifecycle_recovery_tests::workspace_for(&agent, now);
        workspace.name = format!("project-{number}");
        workspace.path = format!("/home/cma/project-{number}");
        workspace.profile.content.name = workspace.name.clone();
        workspace.profile.content.location = WorkspaceLocation::Directory {
            name: workspace.name.clone(),
        };
        workspace.profile.digest =
            layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
        f.harness.store.create_workspace(workspace).await.unwrap();
        children.push(agent);
    }
    *f.backend.retirement_home.lock().unwrap() = Some(f.homes.clone());
    (f, active, children, now)
}
pub(super) async fn admit(f: &Fixture, active: &Sandbox) {
    let parent = f
        .harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    assert_eq!(parent.status, SandboxStatus::Retiring);
    assert_eq!(parent.work.step, SandboxWorkStep::StopAgents);
}
pub(super) fn parent_worker(f: &Fixture) -> Arc<SandboxWorker> {
    parent_worker_inner(f, None)
}
pub(super) fn finalizing_parent_worker(
    f: &Fixture,
    clock: Arc<Mutex<SystemTime>>,
) -> Arc<SandboxWorker> {
    parent_worker_with_retirement_admission(f, clock, true)
}
pub(super) fn parent_worker_with_retirement_admission(
    f: &Fixture,
    clock: Arc<Mutex<SystemTime>>,
    admission_enabled: bool,
) -> Arc<SandboxWorker> {
    let service = crate::sandbox::native_retirement_finalizer::NativeRetirementFinalizer::new(
        f.harness.store.clone(),
        f.harness.adapter.clone(),
        "test".into(),
        f.backend.retirement_records.clone(),
        admission_enabled,
    )
    .with_clock(Arc::new(move || *clock.lock().unwrap()));
    parent_worker_inner(f, Some(Arc::new(service)))
}
fn parent_worker_inner(
    f: &Fixture,
    finalizer: Option<Arc<crate::sandbox::native_retirement_finalizer::NativeRetirementFinalizer>>,
) -> Arc<SandboxWorker> {
    let mut config = SandboxWorkerConfig::new(
        "retirement-parent",
        "test",
        runtime_image(),
        f.harness.policy.clone(),
    )
    .with_home_policy(Some(f.home_policy()));
    config.retry_delay = Duration::from_secs(5);
    config.retirement_finalizer = finalizer;
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: f.harness.store.clone(),
            adapter: f.harness.adapter.clone(),
            credentials: f.harness.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: f.harness.actions.clone(),
            }),
            agents: Arc::new(
                crate::sandbox::live_agent_stopper::DurableLiveAgentStopper::new(
                    f.harness.store.clone(),
                    f.harness.registry.clone(),
                ),
            ),
            registry: f.harness.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(f.harness.store.clone()),
            operations: AgentOperationService::new(
                f.harness.store.clone(),
                f.harness.registry.clone(),
            ),
        },
        config,
    )
}
pub(super) fn child_worker(
    f: &Fixture,
    owner: Option<Arc<OwnershipCoordinator>>,
    name: &str,
) -> Arc<AgentWorker> {
    configured_child_worker(
        f,
        AgentWorkerConfig::new(name, "test").with_ownership(owner),
    )
}

pub(super) fn configured_child_worker(
    f: &Fixture,
    mut config: AgentWorkerConfig,
) -> Arc<AgentWorker> {
    config.retry_delay = Duration::from_secs(5);
    AgentWorker::new(
        AgentWorkerDependencies {
            store: f.harness.store.clone(),
            adapter: f.harness.adapter.clone(),
            skills: Arc::new(crate::sandbox::agent_skills::EmptyAgentSkillResolver),
            registry: f.harness.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(f.harness.store.clone()),
            operations: AgentOperationService::new(
                f.harness.store.clone(),
                f.harness.registry.clone(),
            ),
        },
        config,
    )
}
pub(super) async fn blocked(f: &Fixture, active: &Sandbox, children: &[Agent]) {
    let parent = f.harness.stored(&active.id).await;
    assert_eq!(parent.status, SandboxStatus::Retiring);
    assert_eq!(parent.work.step, SandboxWorkStep::StopAgents);
    assert_eq!(parent.runtime, active.runtime);
    assert_eq!(parent.lifecycle.home, active.lifecycle.home);
    assert_eq!(parent.credential, active.credential);
    assert!(!f
        .backend
        .calls
        .snapshot()
        .iter()
        .any(|c| c.starts_with("delete")));
    assert!(!f
        .harness
        .actions
        .snapshot()
        .iter()
        .any(|c| c.starts_with("revoke")));
    assert_eq!(f.backend.snapshot().await.agents.len(), 2);
    for child in children {
        let stored = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
        assert!(stored.status.is_live());
        assert_eq!(stored.work.step, AgentWorkStep::CloseProcess);
    }
}
pub(super) async fn report(
    case: &str,
    f: &Fixture,
    id: &SandboxId,
    children: &[Agent],
    extra: serde_json::Value,
) {
    let parent = f.harness.stored(id).await;
    let mut states = Vec::new();
    for child in children {
        let stored = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
        states.push(json!({"status":format!("{:?}",stored.status),"step":format!("{:?}",stored.work.step),"failure_code":stored.failure.as_ref().map(|f|f.class.clone())}));
    }
    println!(
        "RETIREMENT_CASE6 {}",
        json!({"case":case,"parent_status":format!("{:?}",parent.status),"parent_step":format!("{:?}",parent.work.step),"parent_failure_code":parent.failure.as_ref().map(|f|f.class.clone()),"children":states,"home_released":parent.lifecycle.home.as_ref().map(|h|h.released),"runtime_bound":parent.runtime.is_some(),"credential_retained":parent.credential.current.is_some(),"extra":extra})
    );
}

pub(super) struct Owners {
    pub store: Arc<MongoAgentOwnership>,
    pub coordinators: Vec<Arc<OwnershipCoordinator>>,
    client: mongodb::Client,
    name: String,
}
impl Owners {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let name = format!("retirement6_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
        let mut coordinators = Vec::new();
        for i in 0..2 {
            coordinators.push(
                OwnershipCoordinator::register(
                    store.clone(),
                    Member {
                        instance_id: format!("retirement-am-{i}"),
                        protocol: PROTOCOL_VERSION,
                        relay_address: format!("http://127.0.0.1:{}", 31001 + i),
                        expires_at_ms: 0,
                        draining: false,
                        owned_agents: 0,
                        max_owned_agents: 256,
                    },
                )
                .await
                .unwrap(),
            );
        }
        Self {
            store,
            coordinators,
            client,
            name,
        }
    }
    pub async fn acquire(&self, id: &layer2_core::AgentId) -> Arc<Authority> {
        for c in &self.coordinators {
            if let Some(a) = c.acquire(id).await.unwrap() {
                return a;
            }
        }
        panic!("no eligible owner")
    }
    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
pub(super) struct Receiver {
    pub calls: Arc<AtomicUsize>,
    pub recover_after: Arc<AtomicUsize>,
    pub server: tokio::task::JoinHandle<()>,
}
impl Receiver {
    pub async fn new(f: &Fixture, parent: &Sandbox) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let endpoint = address.clone();
        let records = f.backend.retirement_records.clone();
        let owner = parent.owner.clone();
        let binding = parent.runtime.clone().unwrap();
        *f.backend.retirement_receiver_url.lock().unwrap() = Some(address.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let recover_after = Arc::new(AtomicUsize::new(usize::MAX));
        let recovery = recover_after.clone();
        let router = axum::Router::new()
            .route(
                "/v1/sandboxes/{runtime}/endpoints/{port}",
                axum::routing::get(move || {
                    let endpoint = endpoint.clone();
                    async move { axum::Json(json!({"endpoint":endpoint})) }
                }),
            )
            .route(
                "/runtime/agents/{agent}/manager-ownership/{action}",
                axum::routing::post(
                    move |axum::extract::Path((agent, action)): axum::extract::Path<(
                        String,
                        String,
                    )>,
                          headers: axum::http::HeaderMap,
                          axum::Json(transport_owner): axum::Json<
                        layer3_adapter::owner_protocol::TransportOwner,
                    >| {
                        assert_eq!(action, "challenge");
                        assert_eq!(transport_owner.agent_id, agent);
                        assert_eq!(headers["open-sandbox-api-key"], "local-fixture-only");
                        transport_owner.validate().unwrap();
                        let sequence = counter.fetch_add(1, Ordering::SeqCst);
                        *records.observation.lock().unwrap() = Some(
                            crate::store::native_retirement_action::RetirementObservation {
                                owner: owner.clone(),
                                binding: binding.clone(),
                                receiver: layer3_adapter::receiver_failure::RetirementReceiver {
                                    runtime: RuntimeHandle::new(&binding.runtime_id),
                                    pod: super::records::pod(),
                                    owner: transport_owner,
                                },
                            },
                        );
                        let healthy = sequence >= recovery.load(Ordering::SeqCst);
                        async move {
                            use axum::response::IntoResponse;
                            if healthy {
                                axum::Json(json!({
                                "protocol": PROTOCOL_VERSION,
                                "nonce": uuid::Uuid::new_v4(),
                                "valid_for_ms": layer3_adapter::owner_protocol::RECEIVER_LEASE_MS,
                            })).into_response()
                            } else {
                                axum::http::StatusCode::NOT_FOUND.into_response()
                            }
                        }
                    },
                ),
            );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let transport = Arc::new(
            layer3_adapter::hosted::ProviderHostedControlTransport::for_retained_opensandbox(
                layer3_adapter::provider::SandboxClient::new(&address, "local-fixture-only"),
            )
            .unwrap(),
        );
        *f.backend.close_transport.lock().unwrap() = Some(transport);
        Self {
            calls,
            recover_after,
            server,
        }
    }
}
