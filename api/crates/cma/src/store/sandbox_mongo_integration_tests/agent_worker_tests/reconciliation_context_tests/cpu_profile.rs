//! Manual, content-free measurements against an owned disposable Mongo fixture.
use super::*;
use layer2_core::resource_access::{ResourceAccessStore, ResourceTarget};
use layer2_core::sandbox::store::SandboxReconciliationStore;
use mongodb::event::cmap::CmapEvent;
use std::time::{Duration, Instant};

mod matched_ownership;
mod population;
mod stream_profile;

struct Meter {
    checkouts: Arc<AtomicU64>,
}

impl Meter {
    async fn phase<F, Fut>(&self, label: &str, iterations: u32, mut action: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let before = self.checkouts.load(Ordering::SeqCst);
        let cpu = cpu_seconds();
        let started = Instant::now();
        for _ in 0..iterations {
            action().await;
        }
        let elapsed = started.elapsed().as_secs_f64();
        let consumed = cpu_seconds() - cpu;
        let checkouts = self.checkouts.load(Ordering::SeqCst) - before;
        println!(
            "CPU_PROFILE {}",
            serde_json::json!({
                "phase": label, "iterations": iterations, "wall_seconds": elapsed,
                "cpu_seconds": consumed, "mean_cores": consumed / elapsed,
                "checkouts": checkouts, "checkouts_per_iteration": checkouts as f64 / f64::from(iterations),
                "wall_ms_per_iteration": elapsed * 1000. / f64::from(iterations),
                "cpu_ms_per_iteration": consumed * 1000. / f64::from(iterations),
            })
        );
    }
}

// ps reports this Rust process's accumulated user + system CPU, excluding Mongo.
// macOS resolution is 10 ms; measurements run serially to avoid cross-phase work.
fn cpu_seconds() -> f64 {
    let result = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(result.status.success());
    let value = String::from_utf8(result.stdout).unwrap();
    value
        .trim()
        .split(':')
        .fold(0., |total, part| total * 60. + part.parse::<f64>().unwrap())
}

#[tokio::test]
#[ignore = "manual CPU profile; requires an owned disposable replica set"]
async fn production_shaped_idle_profile() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("cma_cpu_{}", uuid::Uuid::new_v4().simple());
    let checkouts = Arc::new(AtomicU64::new(0));
    let captured = checkouts.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    // CMAP only: command callbacks force full reply decoding in the driver.
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            captured.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let agents = population::seed(&store).await;
    let registry = Arc::new(LiveAgentRegistry::default());
    let updates = DurableAgentUpdateRecorder::new(store.clone());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates,
            operations: AgentOperationService::new(store.clone(), registry.clone()),
        },
        AgentWorkerConfig::new("cpu-profile", "cpu-profile").with_managed_documents(true),
    );
    let mut senders = Vec::new();
    for agent in agents
        .iter()
        .filter(|a| a.work.step == AgentWorkStep::None && a.status != AgentStatus::Paused)
    {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        senders.push(sender);
        let attachment = registry
            .attach(
                agent.id.clone(),
                agent.conversation.conversation_id.clone().unwrap(),
                1,
                agent.work.next_fence - 1,
                RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                ConversationInputSender::spawn(1, |_| async { Ok(()) }),
                receiver,
                Arc::new(IdleRecorder),
            )
            .unwrap();
        registry.publish_and_activate(&attachment).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let meter = Meter { checkouts };
    meter
        .phase("agent_reconciliation_50_candidates", 100, || async {
            assert_eq!(
                worker
                    .reconcile_connections(SystemTime::now())
                    .await
                    .unwrap(),
                0
            );
        })
        .await;
    meter
        .phase("agent_due_discovery", 200, || async {
            store.due_agent_work(MINUTE, 50).await.unwrap();
        })
        .await;
    meter
        .phase("sandbox_due_discovery", 200, || async {
            store.due_sandbox_work(MINUTE, 50).await.unwrap();
        })
        .await;
    meter
        .phase("workspace_due_discovery", 200, || async {
            store.due_workspace_work(MINUTE, 50).await.unwrap();
        })
        .await;
    meter
        .phase("sandbox_authority_339", 40, || async {
            let mut cursor = None;
            let mut total = 0;
            loop {
                let page = store
                    .sandbox_runtime_authority(&PageRequest::new(MAX_PAGE_SIZE, cursor).unwrap())
                    .await
                    .unwrap();
                total += page.items.len();
                cursor = page.next;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(total, 339);
        })
        .await;
    let target = ResourceTarget::Agent {
        agent_id: agents[0].id.clone(),
    };
    meter
        .phase("one_resource_authority_read", 100, || async {
            store.resource_access_snapshot(&target).await.unwrap();
        })
        .await;
    meter
        .phase("one_agent_log_read", 100, || async {
            store
                .agent_log(&agents[0].id, 0, MAX_PAGE_SIZE)
                .await
                .unwrap();
        })
        .await;
    stream_profile::measure(&store, &agents[0], &meter).await;
    client.database(&name).drop().await.unwrap();
}
