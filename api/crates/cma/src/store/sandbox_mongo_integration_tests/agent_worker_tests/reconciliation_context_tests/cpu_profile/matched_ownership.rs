use super::stream_profile::NoExternalCalls;
use super::*;
use crate::sandbox::binding_lifecycle_test_support::ConnectionSweep;
use layer1_public::{
    agent_page::AgentPageOrigin,
    rest::{agent_router, AgentRoutesState},
};
use layer2_core::agent_ownership::{
    MaintenancePolicy, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};
use layer2_core::{
    profiles::InMemoryProfileStore,
    resource_access::ResourceAccessService,
    sandbox::{AgentReadService, AgentService},
};
use layer3_adapter::ConversationUpdate;

mod timing;
use timing::{measure_window, TimingMeter};

struct HttpForeground {
    address: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for HttpForeground {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl HttpForeground {
    async fn start(manager: &Manager, client: &Client, database: &str, owner: &SubjectId) -> Self {
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: owner.to_string(),
            },
            None,
        );
        let store = manager.store.clone();
        let access = Arc::new(ResourceAccessService::new(store.clone(), auth.clone()));
        let reads = AgentReadService::new(store.clone(), Arc::new(NoExternalCalls))
            .with_resource_access(access);
        let agents = AgentService::new(
            store.clone(),
            Arc::new(InMemoryProfileStore::default()),
            manager.registry.clone(),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        );
        let operations = AgentOperationService::new(store, manager.registry.clone());
        let records = Arc::new(
            crate::store::operation::MongoOperationStore::open_for_test(client, database)
                .await
                .unwrap(),
        );
        let app = agent_router(AgentRoutesState::new(
            AgentPageOrigin::new("https://measurement.example.test").unwrap(),
            agents,
            reads,
            operations,
            records,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ))
        .layer(axum::middleware::from_fn(
            crate::background::interactive_request,
        ))
        .layer(axum::middleware::from_fn(timing::foreground));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { address, task }
    }
}

struct Manager {
    coordinator: Arc<OwnershipCoordinator>,
    baseline: Arc<AgentWorker>,
    deferred: Arc<AgentWorker>,
    registry: Arc<LiveAgentRegistry>,
    store: Arc<MongoSandboxStore>,
}

async fn measured_client(uri: &str, meter: &Meter, timings: &TimingMeter) -> Client {
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let checkouts = meter.checkouts.clone();
    let waits = timings.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| match event {
        CmapEvent::ConnectionCheckedOut(event) => {
            checkouts.fetch_add(1, Ordering::SeqCst);
            waits.record(event.duration, false);
        }
        CmapEvent::ConnectionCheckoutFailed(event) => waits.record(event.duration, true),
        _ => {}
    }));
    // Command monitoring forces reply decoding. Both pinned implementations
    // use exactly this instrumentation; retain only command name and timing.
    let commands = timings.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        use mongodb::event::command::CommandEvent;
        match event {
            CommandEvent::Succeeded(event) => {
                commands.command(&event.command_name, event.duration, false)
            }
            CommandEvent::Failed(event) => {
                commands.command(&event.command_name, event.duration, true)
            }
            _ => {}
        }
    }));
    Client::with_options(options).unwrap()
}

impl Manager {
    async fn start(client: &Client, database: &str, id: String) -> Self {
        let store = Arc::new(
            MongoSandboxStore::open(client, database)
                .await
                .unwrap()
                .enable_agent_ownership(id.clone())
                .await
                .unwrap(),
        );
        let coordinator = OwnershipCoordinator::prepare_with_policy_options(
            store.agent_ownership.as_ref().unwrap().clone(),
            Member {
                instance_id: id.clone(),
                protocol: PROTOCOL_VERSION,
                relay_address: "http://fixture.invalid:8080".into(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 64,
            },
            true,
            MaintenancePolicy::Legacy,
            true,
        );
        coordinator.activate().await.unwrap();
        let registry = Arc::new(LiveAgentRegistry::default());
        registry.install_ownership(coordinator.clone());
        let worker = |defer| {
            AgentWorker::new(
                AgentWorkerDependencies {
                    store: store.clone(),
                    adapter: Arc::new(CodexAdapter::with_backend(
                        InMemoryHostedBackend::default(),
                        CodexSandboxSpec::default(),
                    )),
                    skills: Arc::new(EmptyAgentSkillResolver),
                    updates: DurableAgentUpdateRecorder::new(store.clone()),
                    operations: AgentOperationService::new(store.clone(), registry.clone()),
                    registry: registry.clone(),
                },
                AgentWorkerConfig::new(&id, &id)
                    .with_ownership(Some(coordinator.clone()))
                    .with_managed_documents(true)
                    .with_skip_ineligible_reconciliation(true)
                    .with_defer_foreign_reconciliation(defer),
            )
        };
        Self {
            baseline: worker(false),
            deferred: worker(true),
            coordinator,
            registry,
            store,
        }
    }

    fn attach(&self, agent: &Agent) -> tokio::sync::mpsc::Sender<ConversationUpdate> {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let attachment = self
            .registry
            .attach(
                agent.id.clone(),
                agent.conversation.conversation_id.clone().unwrap(),
                agent.conversation.runtime_generation.unwrap(),
                agent.work.next_fence - 1,
                RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                ConversationInputSender::spawn(1, |_| async { Ok(()) }),
                receiver,
                Arc::new(IdleRecorder),
            )
            .unwrap();
        self.registry.publish_and_activate(&attachment).unwrap();
        sender
    }
}

// Each mode visits the same live rows with the same real, renewable leases
// and attached streams. Independently composed managers share only Mongo.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual paired scheduler measurement against a disposable replica set"]
async fn matched_live_ownership_profile() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let phase_ms = std::env::var("CMA_PROFILE_HTTP_PHASE_MS")
        .map(|value| value.parse::<u64>().unwrap())
        .unwrap_or(0);
    assert!(phase_ms < 125);
    for replicas in [1, 2] {
        let meter = Meter {
            checkouts: Arc::new(AtomicU64::new(0)),
        };
        let timings = TimingMeter::default();
        let name = format!("cma_matched_{}", uuid::Uuid::new_v4().simple());
        let client = measured_client(&uri, &meter, &timings).await;
        let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
        let agents = population::seed(&fixture).await;
        let mut managers = Vec::new();
        for index in 0..replicas {
            let client = measured_client(&uri, &meter, &timings).await;
            managers.push(Manager::start(&client, &name, format!("owner-{index}")).await);
        }

        let mut streams = Vec::new();
        let mut custody = vec![0; replicas];
        for agent in &agents[..33] {
            let mut attached = false;
            for (index, manager) in managers.iter().enumerate() {
                if let Some(authority) = manager.coordinator.acquire(&agent.id).await.unwrap() {
                    streams.push(
                        authority
                            .run(async { manager.attach(agent) })
                            .await
                            .unwrap(),
                    );
                    custody[index] += 1;
                    attached = true;
                    break;
                }
            }
            assert!(attached);
        }
        assert!(custody.iter().all(|count| *count > 0));
        assert_eq!(streams.len(), 33);
        println!(
            "MATCHED_COHORT replicas={replicas} live={} custody={custody:?}",
            agents.len()
        );
        let mut servers = Vec::new();
        for manager in &managers {
            servers.push(HttpForeground::start(manager, &client, &name, &agents[0].owner).await);
        }
        let http = timing::LoadGenerator::start(&servers, agents[0].id.clone());
        for foreground in [false, true] {
            for defer in [false, true, true, false] {
                measure_window(
                    &meter, &timings, &managers, &http, defer, foreground, phase_ms,
                )
                .await;
            }
        }
        for agent in &agents {
            assert_eq!(
                fixture.get_agent(&agent.id).await.unwrap().as_ref(),
                Some(agent)
            );
        }
        for (manager, count) in managers.iter().zip(custody) {
            assert_eq!(manager.coordinator.owned().await.len(), count);
        }
        drop(http);
        drop(servers);
        drop(streams);
        drop(managers);
        client.database(&name).drop().await.unwrap();
    }
}

fn rss_kib() -> u64 {
    let result = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}
