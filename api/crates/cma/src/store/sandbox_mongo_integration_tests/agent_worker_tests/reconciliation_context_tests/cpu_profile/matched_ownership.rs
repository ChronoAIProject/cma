use super::*;
use crate::sandbox::binding_lifecycle_test_support::ConnectionSweep;
use futures_util::future::join_all;
use layer2_core::agent_ownership::{
    MaintenancePolicy, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};
use layer3_adapter::ConversationUpdate;

struct Manager {
    coordinator: Arc<OwnershipCoordinator>,
    baseline: Arc<AgentWorker>,
    deferred: Arc<AgentWorker>,
    registry: Arc<LiveAgentRegistry>,
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
        let target = ResourceTarget::Agent {
            agent_id: agents[0].id.clone(),
        };
        for foreground in [false, true] {
            for defer in [false, true, true, false] {
                measure_window(&meter, &fixture, &managers, &target, defer, foreground).await;
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
        drop(streams);
        drop(managers);
        client.database(&name).drop().await.unwrap();
    }
}

async fn measure_window(
    meter: &Meter,
    fixture: &MongoSandboxStore,
    managers: &[Manager],
    target: &ResourceTarget,
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
        if foreground {
            for index in 0..64 {
                tokio::time::sleep_until((started + Duration::from_millis(index * 125)).into())
                    .await;
                let requested = Instant::now();
                fixture.resource_access_snapshot(target).await.unwrap();
                latencies.push(requested.elapsed().as_secs_f64() * 1000.);
            }
        }
        latencies.sort_by(f64::total_cmp);
        latencies
    };
    let ((), reads) = tokio::join!(background, reads);
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
            "store_read_p95_ms": reads.get(reads.len().saturating_mul(95).div_ceil(100).saturating_sub(1)),
        })
    );
}
