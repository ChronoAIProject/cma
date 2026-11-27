use super::cross_interface_tests::{authenticated, read, Route};
use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};
use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, AgentResponseStatus};
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome, AgentLogPublicationIdentity,
};
use tower::ServiceExt;

fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: "http://loopback.fixture".into(),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 8,
    }
}

#[tokio::test]
async fn claimed_response_recovers_missing_receiver_after_owner_handoff_once() {
    let owners = Arc::new(MemoryOwnershipStore::default());
    let old = OwnershipCoordinator::register(owners.clone(), member("first-manager"))
        .await
        .unwrap();
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted_agent = harness.admit("receiver-handoff", vec![]).await;
    harness.registry.install_ownership(old.clone());
    let first_owner = old.acquire(&admitted_agent.id).await.unwrap().unwrap();
    let agent = first_owner
        .run(harness.drive_ready(&admitted_agent.id))
        .await
        .unwrap();
    let next = OwnershipCoordinator::register(owners, member("next-manager"))
        .await
        .unwrap();
    let successor_store = harness.store.clone();
    exercise(harness, agent, &old, &first_owner, next, successor_store).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segmented_claimed_response_recovers_missing_receiver_after_owner_handoff_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_receiver_handoff_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_segment_adoption_enabled(true)
            .enable_agent_ownership("first-manager".into())
            .await
            .unwrap(),
    );
    let old = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        member("first-manager"),
    )
    .await
    .unwrap();
    let successor_store = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_segment_adoption_enabled(true)
            .enable_agent_ownership("next-manager".into())
            .await
            .unwrap(),
    );
    let mut harness = Harness::with_store(store, Arc::new(EmptyAgentSkillResolver), 1).await;
    harness.now = SystemTime::now();
    let admitted_agent = harness.admit("segmented-receiver-handoff", vec![]).await;
    harness.registry.install_ownership(old.clone());
    let first_owner = old.acquire(&admitted_agent.id).await.unwrap().unwrap();
    let ready = first_owner
        .run(harness.drive_ready(&admitted_agent.id))
        .await
        .unwrap();
    crate::store::skill_result_test_support::enable_segments(&harness.store, &ready).await;
    first_owner
        .run(harness.store.append_agent_log_keyed(
            &ready.id,
            &AgentLogPublicationIdentity {
                key: "receiver-handoff-adoption".into(),
                fingerprint: [7; 32],
                appended_at: layer2_core::now_rfc3339(),
            },
            AgentLogKind::AvailableCommandsSet { commands: vec![] },
        ))
        .await
        .unwrap()
        .unwrap();
    crate::store::skill_result_test_support::assert_format(&harness.store, &ready.id, true).await;
    let agent = harness.stored(&ready.id).await;
    let next = OwnershipCoordinator::register(
        successor_store.agent_ownership.as_ref().unwrap().clone(),
        member("next-manager"),
    )
    .await
    .unwrap();
    exercise(
        harness,
        agent.clone(),
        &old,
        &first_owner,
        next,
        successor_store.clone(),
    )
    .await;
    crate::store::skill_result_test_support::assert_format(&successor_store, &agent.id, true).await;
    client.database(&name).drop().await.unwrap();
}

async fn admitted<S: FixtureStore>(harness: &Harness<S>) -> AdmittedAgentCommand {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page = harness
                .store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap();
            if let Some(command) = page.items.into_iter().next() {
                return command;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("public command admitted")
}

async fn exercise<S: FixtureStore>(
    mut harness: Harness<S>,
    agent: Agent,
    old: &OwnershipCoordinator,
    first_owner: &Arc<layer2_core::agent_ownership::Authority>,
    next: Arc<OwnershipCoordinator>,
    successor_store: Arc<S>,
) {
    let target = AgentRuntimeTarget {
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
    };
    let old_guard = first_owner
        .run(async {
            layer2_core::sandbox::reconnect_guard_for_test(harness.registry.clone(), target.clone())
        })
        .await
        .unwrap();
    old.begin_drain().await;
    old.handoff(first_owner, async {
        harness.registry.detach_owner(&first_owner.token);
        Ok(())
    })
    .await
    .unwrap();

    harness.store = successor_store;
    harness.updates = DurableAgentUpdateRecorder::new(harness.store.clone());
    harness.registry = Arc::new(LiveAgentRegistry::default());
    harness.registry.install_ownership(next.clone());
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        AgentWorkerConfig::new("next-agent-worker", DEPLOYMENT_ID)
            .with_ownership(Some(next.clone())),
    );
    // A missing receiver without current ownership cannot change durable work.
    AgentRuntimeRegistry::recover_missing_attachment(harness.registry.as_ref(), &target).await;
    assert_eq!(harness.stored(&agent.id).await, agent);
    assert!(first_owner
        .run(AgentRuntimeRegistry::recover_missing_attachment(
            harness.registry.as_ref(),
            &target
        ))
        .await
        .is_err());
    assert_eq!(harness.stored(&agent.id).await, agent);

    let probe = runtime_tests::RuntimeProbe::gated();
    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "handoff-command-worker")
            .await;
    let worker = worker.with_ownership(Some(next.clone()));
    let router = authenticated(router, &harness);
    let pending = tokio::spawn({
        let router = router.clone();
        let request = Route::ResponsesV2.request(&agent.id, "continue after owner handoff");
        async move { router.oneshot(request).await.unwrap() }
    });
    let command = admitted(&harness).await;
    let id = &command.receipt.envelope.command_id;
    worker.run_scheduled(id).await.unwrap();
    let queued = harness.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(queued.status, AgentResponseStatus::Queued);
    assert_eq!(queued.receipt, command.receipt);
    assert_eq!(queued.queue_deadline, command.queue_deadline);
    assert!(queued.work.dispatch.is_none() && queued.work.lease.is_none());
    assert!(probe.inputs().is_empty());
    let recovering = harness.stored(&agent.id).await;
    assert_eq!(recovering.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(recovering.conversation, agent.conversation);

    assert!(harness
        .store
        .due_agent_work(&layer2_core::now_rfc3339(), 32)
        .await
        .unwrap()
        .contains(&agent.id));
    harness.run(&agent.id, SystemTime::now()).await;
    let recovered = harness.stored(&agent.id).await;
    assert_eq!(recovered.work.step, AgentWorkStep::None);
    assert_eq!(
        recovered.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        recovered.conversation.runtime_generation,
        agent.conversation.runtime_generation
    );
    let current_owner = next.acquire(&agent.id).await.unwrap().unwrap();
    harness.registry.fence(
        &agent.id,
        Some(target.runtime_generation),
        recovered.work.next_fence - 1,
    );
    let (observed, observed_rx) = oneshot::channel();
    let (resume, resume_rx) = oneshot::channel();
    *harness.registry.before_missing_attachment.lock().unwrap() = Some((observed, resume_rx));
    let delayed = tokio::spawn({
        let registry = harness.registry.clone();
        let target = target.clone();
        let owner = current_owner.clone();
        async move {
            owner
                .run(AgentRuntimeRegistry::recover_missing_attachment(
                    registry.as_ref(),
                    &target,
                ))
                .await
                .unwrap()
        }
    });
    tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .unwrap()
        .unwrap();
    let candidate = AgentConnectionRecoveryCandidate::from_agent(&recovered).unwrap();
    let request = candidate.restart_request(layer2_core::now_rfc3339());
    let scheduled = current_owner
        .run(harness.store.schedule_agent_connection_recovery(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scheduled, AgentConnectionRecoveryOutcome::Scheduled);
    harness.run(&agent.id, SystemTime::now()).await;
    let published = harness.stored(&agent.id).await;
    assert_eq!(published.work.step, AgentWorkStep::None);
    resume.send(()).unwrap();
    delayed.await.unwrap();
    assert_eq!(
        harness.stored(&agent.id).await,
        published,
        "a concurrent publication wins after the initial missing-receiver observation"
    );
    worker.run_scheduled(id).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-cma-command-id"], id.as_str());
    drop(response);
    let dispatched = harness.store.get_command(id).await.unwrap().unwrap();
    assert!(dispatched.work.dispatch.is_some());
    assert_eq!(dispatched.receipt, command.receipt);
    assert_eq!(dispatched.queue_deadline, command.queue_deadline);
    assert_eq!(probe.inputs().len(), 1);
    assert_eq!(probe.inputs()[0].0, target);
    probe.release_reply();

    let response_id = command.receipt.response_id.as_ref().unwrap();
    let path = format!("/api/v2/responses/{response_id}");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response: Value = serde_json::from_str(&read(&router, &path).await).unwrap();
            if response["status"] == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    worker.run_scheduled(id).await.unwrap();
    worker.run_scheduled(id).await.unwrap();
    let done = harness.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(done.status, AgentResponseStatus::Completed);
    assert_eq!(done.receipt, command.receipt);
    assert_eq!(done.work.dispatch, dispatched.work.dispatch);
    assert_eq!(probe.inputs().len(), 1);
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(
        ready.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(logs.len() as u64, ready.conversation.last_seq);
    assert!(logs
        .iter()
        .enumerate()
        .all(|(index, entry)| entry.seq == index as u64 + 1));
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
            .count(),
        1
    );

    // An unscoped call and a stale guard have no authority over the replacement.
    assert_eq!(
        AgentRuntimeRegistry::request_reconnect(harness.registry.as_ref(), &target).await,
        Err(AgentRuntimeError::StaleGeneration)
    );
    drop(old_guard);
    tokio::task::yield_now().await;
    assert_eq!(harness.stored(&agent.id).await, ready);
    assert!(
        AgentRuntimeRegistry::resolve(harness.registry.as_ref(), &target)
            .await
            .is_ok()
    );

    // This is the guard used by cancelled settings and managed context writes.
    // Dropping it outside the scope must re-enter its captured authority.
    let guard = current_owner
        .run(async {
            layer2_core::sandbox::reconnect_guard_for_test(harness.registry.clone(), target.clone())
        })
        .await
        .unwrap();
    drop(guard);
    let scheduled = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let agent = harness.stored(&agent.id).await;
            if agent.work.step == AgentWorkStep::PublishConnection {
                break agent;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(scheduled.revision, ready.revision + 1);
    assert_eq!(scheduled.conversation, ready.conversation);
}
