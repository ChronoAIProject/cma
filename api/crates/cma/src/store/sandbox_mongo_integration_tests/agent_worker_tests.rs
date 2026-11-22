use super::*;

mod empty_discovery_tests;
mod reconciliation_context_tests;
mod work_retry_policy_tests;

async fn idle_connection_fixture(
    store: &MongoSandboxStore,
    suffix: &str,
    status: AgentStatus,
) -> Agent {
    let mut agent = ready_conversation_fixture(store, suffix).await;
    let expected = agent.revision;
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    agent.status = status;
    agent.conversation.active_turn =
        matches!(status, AgentStatus::Working | AgentStatus::WaitingApproval)
            .then_some(agent.conversation.last_seq + 1);
    agent.revision += 1;
    agent.validate_conversation().unwrap();
    store.save_agent(expected, agent.clone()).await.unwrap();
    agent
}

fn connection_recovery_request(agent: &Agent) -> AgentConnectionRecoveryRequest {
    AgentConnectionRecoveryRequest {
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
        expected_revision: agent.revision,
        expected_work_step: agent.work.step,
        expected_authority_fence: Some(agent.work.next_fence - 1),
        requested_at: MINUTE.into(),
        cause: AgentConnectionRecoveryCause::UpdateStreamLost,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn live_connection_scan_and_recovery_cas_are_complete_and_idempotent() {
    let (client, name) = database("agent_connection_recovery").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let ready = idle_connection_fixture(&store, "recovery_ready", AgentStatus::Ready).await;
    let working = idle_connection_fixture(&store, "recovery_working", AgentStatus::Working).await;
    let paused = idle_connection_fixture(&store, "recovery_paused", AgentStatus::Paused).await;

    let first = store
        .live_agent_connections(&PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let second = store
        .live_agent_connections(&PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    let mut scanned: Vec<_> = first
        .items
        .into_iter()
        .chain(second.items)
        .map(|candidate| candidate.agent_id)
        .collect();
    scanned.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let mut expected = vec![ready.id.clone(), working.id.clone(), paused.id];
    expected.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    assert_eq!(scanned, expected);

    let request = connection_recovery_request(&working);
    let left = store.schedule_agent_connection_recovery(request.clone());
    let right = store.schedule_agent_connection_recovery(request);
    let (left, right) = tokio::join!(left, right);
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == AgentConnectionRecoveryOutcome::Scheduled)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == AgentConnectionRecoveryOutcome::AlreadyScheduled)
            .count(),
        1
    );
    let scheduled = store.get_agent(&working.id).await.unwrap().unwrap();
    assert_eq!(scheduled.status, AgentStatus::Working);
    assert_eq!(scheduled.conversation, working.conversation);
    assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(scheduled.revision, working.revision + 1);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn connection_recovery_retries_one_retryable_mongo_update_without_double_mutation() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let app_name = format!("cma-connection-recovery-{}-{sequence}", std::process::id());
    let name = format!(
        "cma_812_connection_recovery_{}_{sequence}",
        std::process::id()
    );
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.retry_writes = Some(true);
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = idle_connection_fixture(&store, "retryable_update", AgentStatus::Ready).await;

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 1_i32 },
            "data": {
                "failCommands": ["update"],
                "appName": &app_name,
                "errorCode": 91_i32,
                "closeConnection": false,
            },
        })
        .await
        .unwrap();
    let result = store
        .schedule_agent_connection_recovery(connection_recovery_request(&agent))
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("update failpoint must be disabled before assertions");

    assert_eq!(result.unwrap(), AgentConnectionRecoveryOutcome::Scheduled);
    let scheduled = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(scheduled.revision, agent.revision + 1);
    assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_due_query_is_bounded_sorted_and_supports_lease_takeover() {
    const BEFORE: &str = "2026-09-04T23:59:00Z";

    let (client, name) = database("agent_work_due").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store
        .create_sandbox(sandbox("sbx_agent_worker"))
        .await
        .unwrap();
    let first = agent_for_worker("agt_a_due", NOW);
    let mut held = agent_for_worker("agt_b_held", BEFORE);
    held.work
        .claim("worker-a".into(), BEFORE, MINUTE.into())
        .unwrap();
    let future = agent_for_worker("agt_c_future", MINUTE);
    let mut expired = agent_for_worker("agt_m_expired", BEFORE);
    expired
        .work
        .claim("worker-a".into(), BEFORE, NOW.into())
        .unwrap();
    let last = agent_for_worker("agt_z_due", BEFORE);
    for agent in [&last, &expired, &future, &held, &first] {
        store.create_agent(agent.clone()).await.unwrap();
    }

    assert_eq!(
        store.due_agent_work(NOW, 0).await,
        Err(SandboxStoreError::InvalidPage)
    );
    assert_eq!(
        store.due_agent_work(NOW, MAX_PAGE_SIZE + 1).await,
        Err(SandboxStoreError::InvalidPage)
    );
    assert_eq!(
        store.due_agent_work(NOW, 2).await.unwrap(),
        vec![first.id.clone(), expired.id.clone()]
    );
    assert!(store
        .claim_agent_work(
            &AgentId("agt_missing".into()),
            "worker-b",
            NOW.into(),
            MINUTE.into(),
        )
        .await
        .unwrap()
        .is_none());
    assert!(store
        .claim_agent_work(&future.id, "worker-b", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .is_none());

    let takeover = store
        .claim_agent_work(&expired.id, "worker-b", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    assert!(takeover.work.owns("worker-b", 2));
    assert_eq!(takeover.work.next_fence, 3);
    assert_eq!(takeover.revision, expired.revision + 1);
    assert_eq!(
        store.due_agent_work(NOW, 5).await.unwrap(),
        vec![first.id, last.id]
    );
    store
        .agents
        .update_one(
            doc! { "_id": held.id.as_str() },
            doc! { "$set": { "work_next_fence": 99_i64 } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.get_agent(&held.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_stale_holder_is_rejected_after_lease_takeover() {
    let (client, name) = database("agent_work_stale_holder").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store
        .create_sandbox(sandbox("sbx_agent_worker"))
        .await
        .unwrap();
    let agent = agent_for_worker("agt_stale_holder", NOW);
    let (mut worktree, _) = worktree_and_agent(&agent.sandbox_id);
    worktree.id = agent.worktree_id.clone();
    worktree.agent_id = Some(agent.id.clone());
    store.create_worktree(worktree).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    let stale = store
        .claim_agent_work(&agent.id, "worker-a", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let current = store
        .claim_agent_work(&agent.id, "worker-b", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();
    let stale_fence = stale.work.lease.as_ref().unwrap().fence;
    let mut next = stale.clone();
    next.work
        .advance(
            "worker-a",
            stale_fence,
            AgentWorkStep::MaterializeHome,
            MINUTE.into(),
        )
        .unwrap();
    next.revision += 1;

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: stale.revision,
                holder: "worker-a".into(),
                fence: stale_fence,
                agent: next,
                worktree: None,
                appended_at: MINUTE.into(),
                sandbox_event: None,
                agent_logs: vec![],
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(current));

    client.database(&name).drop().await.unwrap();
}

mod publication_tests;
mod receipt_tests;
mod retired_runtime_tests;
mod rollback_tests;
mod sign_in_tests;

mod bounded_close_retries;

mod typed_fence;
