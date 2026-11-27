use super::cross_interface_tests::{admitted, authenticated, read, Route};
use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};
use layer2_core::sandbox::agent_io::{AgentCommandWorkerStore, AgentResponseStatus};
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
    let next = OwnershipCoordinator::register(owners, member("next-manager"))
        .await
        .unwrap();
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted_agent = harness.admit("receiver-handoff", vec![]).await;
    harness.registry.install_ownership(old.clone());
    let first_owner = old.acquire(&admitted_agent.id).await.unwrap().unwrap();
    let agent = first_owner
        .run(harness.drive_ready(&admitted_agent.id))
        .await
        .unwrap();
    let target = AgentRuntimeTarget {
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
    };
    old.handoff(&first_owner, async {
        harness.registry.detach_owner(&first_owner.token);
        Ok(())
    })
    .await
    .unwrap();

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
    let worker = worker.with_ownership(Some(next));
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
}
