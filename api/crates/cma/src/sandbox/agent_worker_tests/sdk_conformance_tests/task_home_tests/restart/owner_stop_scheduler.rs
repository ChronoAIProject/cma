use super::*;
use crate::projections::UnavailableSandboxResources;
use crate::sandbox::agent_process_proof::RestartObservation;
use crate::sandbox::agent_scheduler::AgentScheduler;
use crate::sandbox::agent_worker_tests::sdk_conformance_tests::cross_interface_tests::authenticated;
use crate::store::adoption_probe::{self, Boundary};
use crate::store::budget_test_fixture::OwnerStopBudgetFixture;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use layer2_core::agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION};
use layer2_core::sandbox::{AgentReadService, AgentRestartService, AgentService};
use layer2_core::Caller;
use mongodb::bson::doc;
use tokio::sync::{watch, Notify};
use tower::ServiceExt;

#[derive(Clone, Copy)]
pub(super) enum Fault {
    OwnershipConflict,
    LostCommitReply,
}

pub(super) async fn register(
    store: &MongoSandboxStore,
    instance: &str,
) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        Member {
            instance_id: instance.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://loopback.fixture".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 8,
        },
    )
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn exercise(
    database: (&mongodb::Client, &mongodb::Database),
    harness: &Harness<MongoSandboxStore>,
    working: &Agent,
    caller: &Caller,
    sandbox: &Sandbox,
    coordinator: &Arc<OwnershipCoordinator>,
    restart_supported: &AtomicBool,
    budget: &OwnerStopBudgetFixture,
    fault: Fault,
) {
    let (client, database) = database;
    let peer = Arc::new(
        MongoSandboxStore::open(client, database.name())
            .await
            .unwrap()
            .enable_agent_ownership("stop-ingress".into())
            .await
            .unwrap(),
    );
    let ingress = register(&peer, "stop-ingress").await;
    let peer_registry = Arc::new(LiveAgentRegistry::default());
    peer_registry.install_ownership(ingress.clone());
    let reads = AgentReadService::new(peer.clone(), Arc::new(UnavailableSandboxResources));
    let agents = AgentService::new(
        peer.clone(),
        Arc::new(InMemoryProfileStore::default()),
        peer_registry.clone(),
        Default::default(),
    );
    let operations = AgentOperationService::new(peer.clone(), peer_registry);
    let operation_store = Arc::new(
        crate::store::MongoOperationStore::open_for_test(client, database.name())
            .await
            .unwrap(),
    );
    let restart = AgentRestartService::new(
        peer,
        Arc::new(RestartObservation {
            adapter: harness.adapter.clone(),
            deployment_id: DEPLOYMENT_ID.into(),
        }),
    );
    let router = authenticated(
        layer1_public::rest::agent_router(
            layer1_public::rest::AgentRoutesState::new(
                layer1_public::agent_page::AgentPageOrigin::new("http://localhost").unwrap(),
                agents,
                reads,
                operations,
                operation_store,
            )
            .with_owner_agent_restart(restart),
        ),
        harness,
    );
    assert!(coordinator.local(&working.id).await.is_some());
    assert!(ingress.local(&working.id).await.is_none());
    let stop = request(&router, &working.id, "stop", "stop-on-peer").await;
    assert_eq!(stop.status(), StatusCode::ACCEPTED);
    let pending = harness.stored(&working.id).await;
    assert_eq!(pending.status, AgentStatus::Working);
    assert_eq!(pending.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(pending.requested_stop_reason, Some(AgentStopReason::Owner));

    let first_fence = fail_first_publication(database, harness, &pending, fault).await;
    budget.assert_held(&harness.store).await;

    let maintenance_owner = coordinator.clone();
    let maintenance = tokio::spawn(async move {
        loop {
            maintenance_owner.maintain().await.unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    let (shutdown, signal) = watch::channel(false);
    let scheduler = AgentScheduler::new(harness.worker.clone(), Arc::new(Notify::new()))
        .with_event_driven_reconciliation(Arc::new(Notify::new()), true)
        .spawn(signal);
    let stopped = wait_for(harness, &working.id, AgentStatus::Stopped).await;
    assert_eq!(stopped.work.step, AgentWorkStep::None);
    assert!(stopped.work.lease.is_none());
    assert!(stopped.conversation.active_turn.is_none());
    let minimum_fence = match fault {
        Fault::OwnershipConflict => first_fence + 3,
        Fault::LostCommitReply => first_fence + 2,
    };
    assert_eq!(stopped.work.next_fence, minimum_fence + 1);
    budget.assert_settled_once(&harness.store).await;
    assert!(stopped
        .work
        .receipts
        .iter()
        .any(|receipt| receipt.step == AgentWorkStep::CloseProcess));
    let retained = harness
        .store
        .get_sandbox(&stopped.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, SandboxStatus::Active);
    assert_eq!(retained.runtime, sandbox.runtime);
    assert_eq!(retained.lifecycle.home, sandbox.lifecycle.home);
    let requests = harness.backend.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::CloseAgent(_)))
            .count(),
        1
    );

    restart_supported.store(true, Ordering::SeqCst);
    let restart = request(&router, &working.id, "restart", "restart-on-peer").await;
    assert_eq!(restart.status(), StatusCode::ACCEPTED);
    let replay = request(&router, &working.id, "restart", "restart-on-peer").await;
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(
        to_bytes(restart.into_body(), 4096).await.unwrap(),
        to_bytes(replay.into_body(), 4096).await.unwrap()
    );

    let resumed = wait_for(harness, &working.id, AgentStatus::Ready).await;
    assert_eq!(resumed.sandbox_id, working.sandbox_id);
    assert_eq!(resumed.worktree_id, working.worktree_id);
    assert_eq!(
        resumed.conversation.conversation_id,
        working.conversation.conversation_id
    );
    let retained = harness
        .store
        .get_sandbox(&resumed.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.lifecycle.home, sandbox.lifecycle.home);
    let authority = coordinator.local(&resumed.id).await.unwrap();
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone());
    authority
        .run(operations.send_message(
            caller,
            &resumed.id,
            "Continue the retained conversation".into(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        harness.stored(&resumed.id).await.status,
        AgentStatus::Working
    );
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
    maintenance.abort();
    assert!(maintenance.await.unwrap_err().is_cancelled());
    budget.assert_settled_once(&harness.store).await;
}

async fn fail_first_publication(
    database: &mongodb::Database,
    harness: &Harness<MongoSandboxStore>,
    pending: &Agent,
    fault: Fault,
) -> u64 {
    let probe = adoption_probe::arm(database.name(), Boundary::BeforeInteractionFenceAuthority);
    let worker = harness.worker.clone();
    let id = pending.id.clone();
    let attempt =
        tokio::spawn(async move { worker.run_one_diagnostic(&id, SystemTime::now()).await });
    tokio::time::timeout(Duration::from_secs(10), probe.reached.notified())
        .await
        .unwrap();
    let claimed = harness.stored(&pending.id).await;
    let lease = claimed.work.lease.as_ref().unwrap();
    assert_eq!(claimed.work.step, AgentWorkStep::FenceInteraction);
    assert!(lease.expires_at <= rfc3339_at(SystemTime::now() + Duration::from_secs(30)));
    let data = match fault {
        Fault::OwnershipConflict => doc! {
            "failCommands": ["update"], "namespace": format!("{}.cma_agent_manager_members", database.name()),
            "errorCode": 112_i32, "errorLabels": Vec::<String>::new(),
        },
        Fault::LostCommitReply => doc! {
            "failCommands": ["commitTransaction"],
            "writeConcernError": {"code": 64_i32, "errmsg": "fixture lost commit acknowledgement"},
            "errorLabels": ["UnknownTransactionCommitResult"],
        },
    };
    let mut data = data;
    data.insert("appName", database.name());
    let admin = database.client().database("admin");
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "alwaysOn", "data": data})
        .await
        .unwrap();
    probe.resume.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(15), attempt).await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let result = result.unwrap().unwrap();
    let persisted = harness.stored(&pending.id).await;
    match fault {
        Fault::OwnershipConflict => {
            let failure = serde_json::to_value(result.unwrap_err().observation).unwrap();
            assert_eq!(failure["phase"], "execution");
            assert_eq!(failure["origin"], "execute");
            assert_eq!(failure["cause_class"], "store_unavailable");
            assert_eq!(persisted, claimed);
        }
        Fault::LostCommitReply => {
            assert!(result.unwrap());
            assert_eq!(persisted.work.step, AgentWorkStep::CloseProcess);
            assert!(persisted.work.lease.is_none());
        }
    }
    lease.fence
}

async fn request(router: &Router, id: &AgentId, action: &str, key: &str) -> Response {
    router
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/agents/{id}/{action}"))
                .header(
                    "authorization",
                    crate::sandbox::agent_worker_tests::sdk_conformance_tests::TEST_BEARER,
                )
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn wait_for(
    harness: &Harness<MongoSandboxStore>,
    id: &AgentId,
    expected: AgentStatus,
) -> Agent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        let agent = harness.stored(id).await;
        if agent.status == expected {
            return agent;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {expected:?}, got {:?} at {:?}, attempts={}, work_lease={}",
            agent.status,
            agent.work.step,
            agent.work.attempts,
            agent.work.lease.is_some()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
