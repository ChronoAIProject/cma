use super::*;
use crate::sandbox::live_agent_stopper::DurableLiveAgentStopper;
use crate::sandbox::native_retirement_delivery::CaptureDelivery;
use crate::sandbox::native_retirement_finalizer::NativeRetirementFinalizer;
use crate::store::native_retirement_action::{RetirementPhase, RetirementRecords};
use layer2_core::sandbox::store::WorktreeStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_capture_worker_delivers_refusal_to_successor_parent() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("retirement_composed_{}", Uuid::new_v4().simple());
    let child_client = Client::with_uri_str(&uri).await.unwrap();
    let parent_client = Client::with_uri_str(&uri).await.unwrap();
    let successor_client = Client::with_uri_str(&uri).await.unwrap();
    let parent_store = Arc::new(
        MongoSandboxStore::open(&parent_client, &name)
            .await
            .unwrap(),
    );
    let successor_store = Arc::new(
        MongoSandboxStore::open(&successor_client, &name)
            .await
            .unwrap(),
    );
    let owners = MongoAgentOwnership::open(&child_client, &name)
        .await
        .unwrap();
    let coordinator = OwnershipCoordinator::register(Arc::new(owners), member())
        .await
        .unwrap();
    let child_store = Arc::new(
        MongoSandboxStore::open(&child_client, &name)
            .await
            .unwrap()
            .enable_agent_ownership("child-owner".into())
            .await
            .unwrap(),
    );
    let (f, mut sandbox, children, _) = active_siblings().await;
    let now = SystemTime::now();
    let mut child = children.into_iter().next().unwrap();
    seed(&f, &parent_store, &mut sandbox, &mut child, now).await;
    let receiver = Receiver::new(&f, &sandbox).await;
    let delivery = CaptureDelivery::new(child_store.clone(), 1);

    let parent = parent_worker(&f, parent_store.clone(), "initial-parent", Some(&delivery));
    let successor = parent_worker(&f, successor_store.clone(), "successor-parent", None);
    let worker = child_worker(
        &f,
        child_store.clone(),
        coordinator.clone(),
        delivery.clone(),
    );

    assert!(parent
        .run_one(&sandbox.id, SystemTime::now())
        .await
        .unwrap());
    let rows = parent_client
        .database(&name)
        .collection::<Document>("sandboxes");
    let row = || {
        rows.find_one(doc! {"_id":sandbox.id.as_str()})
            .read_concern(ReadConcern::majority())
    };
    assert_eq!(phase(&row().await.unwrap().unwrap()), "offered");
    assert!(worker.run_one(&child.id, SystemTime::now()).await.unwrap());
    assert_eq!(
        child_store
            .get_agent(&child.id)
            .await
            .unwrap()
            .unwrap()
            .work
            .step,
        AgentWorkStep::CloseProcess
    );

    let (transport, exchanges, server) = receiver_with_status("404 Not Found").await;
    *f.backend.inspection_transport.lock().unwrap() = Some(transport.clone());
    *f.backend.close_transport.lock().unwrap() = Some(transport);
    let authority = coordinator.acquire(&child.id).await.unwrap().unwrap();
    let result = worker.run_one(&child.id, SystemTime::now()).await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert!(authority.receiver_cancelled());
    delivery.tasks.drain(Duration::from_secs(5)).await.unwrap();
    assert_eq!(phase(&row().await.unwrap().unwrap()), "observed");
    assert_eq!(&*exchanges.lock().unwrap(), &["challenge"]);
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"receiver-close-attempt".into()));
    assert!(successor_store
        .observation(&sandbox.id)
        .await
        .unwrap()
        .is_none());
    *f.backend.inspection_transport.lock().unwrap() = None;
    *f.backend.close_transport.lock().unwrap() = None;

    // A changed receiver Pod must refuse the captured observation.
    f.backend.retirement_incarnation.store(1, Ordering::SeqCst);
    for _ in 0..3 {
        run_due(&successor, &sandbox.id).await;
    }
    assert!(successor_store.action(&sandbox.id).await.unwrap().is_none());
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));
    assert_eq!(
        successor_store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .runtime,
        sandbox.runtime
    );

    f.backend.retirement_incarnation.store(0, Ordering::SeqCst);
    f.backend.hold_finality.store(true, Ordering::SeqCst);
    run_due(&successor, &sandbox.id).await;
    assert_eq!(
        successor_store
            .action(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .phase,
        RetirementPhase::Dispatching
    );
    let pending = successor_store
        .get_sandbox(&sandbox.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, SandboxStatus::Retiring);
    assert!(!pending.lifecycle.home.as_ref().unwrap().released);
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 2);

    f.backend.hold_finality.store(false, Ordering::SeqCst);
    run_due(&successor, &sandbox.id).await;
    let settled = successor_store
        .get_sandbox(&sandbox.id)
        .await
        .unwrap()
        .unwrap();
    assert!(settled.runtime.is_none());
    assert!(successor_store.action(&sandbox.id).await.unwrap().is_none());
    for _ in 0..8 {
        if successor_store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .status
            == SandboxStatus::Retired
        {
            break;
        }
        run_due(&successor, &sandbox.id).await;
    }
    let retired = successor_store
        .get_sandbox(&sandbox.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert!(retired.runtime.is_none());
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(retired.credential.current.is_none());
    assert!(successor_store.action(&sandbox.id).await.unwrap().is_none());
    assert_eq!(
        child_store
            .get_agent(&child.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Stopped
    );
    assert_eq!(
        f.backend
            .calls
            .snapshot()
            .iter()
            .filter(|call| call.as_str() == "retire-alternative")
            .count(),
        1
    );

    receiver.server.abort();
    server.abort();
    parent_client.database(&name).drop().await.unwrap();
}

async fn seed(
    f: &super::super::super::Fixture,
    store: &MongoSandboxStore,
    sandbox: &mut Sandbox,
    child: &mut Agent,
    now: SystemTime,
) {
    sandbox.revision = 1;
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut workspace = f
        .harness
        .store
        .get_workspace(&child.workspace_id)
        .await
        .unwrap()
        .unwrap();
    workspace.revision = 1;
    store.create_workspace(workspace).await.unwrap();
    let mut worktree = f
        .harness
        .store
        .get_worktree(&child.worktree_id)
        .await
        .unwrap()
        .unwrap();
    worktree.revision = 1;
    store.create_worktree(worktree).await.unwrap();
    let at = rfc3339_at(now);
    child.created_at.clone_from(&at);
    child.updated_at.clone_from(&at);
    child.ready_at = Some(at.clone());
    for receipt in &mut child.work.receipts {
        receipt.completed_at.clone_from(&at);
    }
    child.revision = 1;
    child.conversation.last_seq = 0;
    store.create_agent(child.clone()).await.unwrap();
    sandbox.begin_retirement(at).unwrap();
    store.save_sandbox(1, sandbox.clone()).await.unwrap();
}

async fn run_due(worker: &SandboxWorker, sandbox: &SandboxId) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !worker.run_one(sandbox, SystemTime::now()).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("parent becomes eligible for the next guarded attempt");
}

fn parent_worker(
    f: &super::super::super::Fixture,
    store: Arc<MongoSandboxStore>,
    holder: &str,
    admission: Option<&CaptureDelivery>,
) -> Arc<SandboxWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    let adapter = f.harness.adapter.clone();
    let mut config =
        SandboxWorkerConfig::new(holder, "test", runtime_image(), f.harness.policy.clone())
            .with_home_policy(Some(f.home_policy()));
    config.retry_delay = Duration::from_secs(1);
    config.retirement_finalizer = Some(Arc::new(
        NativeRetirementFinalizer::new(
            store.clone(),
            adapter.clone(),
            "test".into(),
            store.clone(),
            true,
        )
        .with_capture(
            store.clone(),
            admission.map(|delivery| delivery.tasks.clone()),
        ),
    ));
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: store.clone(),
            adapter,
            credentials: f.harness.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: f.harness.actions.clone(),
            }),
            agents: Arc::new(DurableLiveAgentStopper::new(
                store.clone(),
                registry.clone(),
            )),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store, registry),
        },
        config,
    )
}

fn child_worker(
    f: &super::super::super::Fixture,
    store: Arc<MongoSandboxStore>,
    coordinator: Arc<OwnershipCoordinator>,
    delivery: CaptureDelivery,
) -> Arc<AgentWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    let mut config =
        AgentWorkerConfig::new("child-owner", "test").with_ownership(Some(coordinator));
    config.capture_delivery = Some(delivery);
    AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: f.harness.adapter.clone(),
            skills: Arc::new(crate::sandbox::agent_skills::EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store, registry),
        },
        config,
    )
}
