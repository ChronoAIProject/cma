use super::stream_profile::NoExternalCalls;
use super::*;
use crate::sandbox::binding_lifecycle_test_support::ConnectionSweep;
use futures_util::future::join_all;
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
        ));
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

async fn measured_client(uri: &str, meter: &Meter) -> Client {
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let checkouts = meter.checkouts.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            checkouts.fetch_add(1, Ordering::SeqCst);
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
#[tokio::test]
#[ignore = "manual paired scheduler measurement against a disposable replica set"]
async fn matched_live_ownership_profile() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    for replicas in [1, 2] {
        let meter = Meter {
            checkouts: Arc::new(AtomicU64::new(0)),
        };
        let name = format!("cma_matched_{}", uuid::Uuid::new_v4().simple());
        let client = measured_client(&uri, &meter).await;
        let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
        let agents = population::seed(&fixture).await;
        let mut managers = Vec::new();
        for index in 0..replicas {
            let client = measured_client(&uri, &meter).await;
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
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let target = &agents[0].id;
        for foreground in [false, true] {
            for defer in [false, true, true, false] {
                measure_window(
                    &meter, &managers, &servers, &http, target, defer, foreground,
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
        drop(servers);
        drop(streams);
        drop(managers);
        client.database(&name).drop().await.unwrap();
    }
}

async fn measure_window(
    meter: &Meter,
    managers: &[Manager],
    servers: &[HttpForeground],
    client: &reqwest::Client,
    target: &AgentId,
    defer: bool,
    foreground: bool,
) {
    let before = meter.checkouts.load(Ordering::SeqCst);
    let cpu = cpu_seconds();
    let started = Instant::now();
    let background = async {
        for index in 0..8 {
            tokio::time::sleep_until((started + Duration::from_secs(index)).into()).await;
            if index % 3 == 0 {
                for manager in managers {
                    manager.coordinator.maintain().await.unwrap();
                }
            }
            for result in join_all(managers.iter().map(|manager| {
                let worker = if defer {
                    &manager.deferred
                } else {
                    &manager.baseline
                };
                async move {
                    let mut sweep = ConnectionSweep::default();
                    let mut slices = 0;
                    loop {
                        slices += 1;
                        assert!(slices <= 50, "reconciliation must complete the cohort");
                        if worker
                            .reconcile_connection_slice(SystemTime::now(), &mut sweep)
                            .await
                            .unwrap()
                        {
                            break;
                        }
                    }
                    slices
                }
            }))
            .await
            {
                assert!(result >= 2, "the live population must span pages");
            }
        }
    };
    let reads = async {
        let mut latencies = Vec::new();
        let mut statuses = std::collections::BTreeMap::new();
        if foreground {
            for index in 0..64 {
                tokio::time::sleep_until((started + Duration::from_millis(index * 125)).into())
                    .await;
                let requested = Instant::now();
                let server = &servers[index as usize % servers.len()];
                let response = client
                    .get(format!("{}/api/v1/agents/{}", server.address, target))
                    .send()
                    .await
                    .unwrap();
                let status = response.status().as_u16();
                let body = response.bytes().await.unwrap();
                latencies.push(requested.elapsed().as_secs_f64() * 1000.);
                *statuses.entry(status).or_insert(0) += 1;
                if status == 200 {
                    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(value["id"], target.as_str());
                }
            }
        }
        latencies.sort_by(f64::total_cmp);
        (latencies, statuses)
    };
    let ((), (reads, statuses)) = tokio::join!(background, reads);
    tokio::time::sleep_until((started + Duration::from_secs(8)).into()).await;
    let elapsed = started.elapsed().as_secs_f64();
    let checkouts = meter.checkouts.load(Ordering::SeqCst) - before;
    println!(
        "MATCHED_SCHEDULER {}",
        serde_json::json!({
            "replicas": managers.len(), "defer": defer, "foreground": foreground,
            "sweeps": 8 * managers.len(), "foreground_reads": reads.len(),
            "elapsed_seconds": elapsed, "checkouts": checkouts,
            "checkouts_per_second": checkouts as f64 / elapsed,
            "mean_cores": (cpu_seconds() - cpu) / elapsed,
            "http_p95_ms": reads.get(reads.len().saturating_mul(95).div_ceil(100).saturating_sub(1)),
            "http_status_counts": statuses,
            "rss_kib": rss_kib(),
        })
    );
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
