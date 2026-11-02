use super::*;

mod reconciliation_context_tests;

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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_publication_atomically_saves_agent_worktree_logs_and_event() {
    let (client, name) = database("agent_work_publication").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "publish").await;
    let claimed = ready_publication_claim(claimed, "conversation-1");
    replace_agent_fixture(&store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    let log_kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-1".into(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    next.conversation.last_seq += u64::try_from(log_kinds.len()).unwrap();

    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: next.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: worktree.clone(),
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: next.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: log_kinds.clone(),
        })
        .await
        .unwrap();

    assert_eq!(store.get_agent(&next.id).await.unwrap(), Some(next.clone()));
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    let logs = store.agent_log(&next.id, 0, 10).await.unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(
        logs.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        logs.iter()
            .map(|entry| entry.kind.clone())
            .collect::<Vec<_>>(),
        log_kinds
    );
    let events = store.sandbox_events(&parent.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].kind,
        SandboxEventKind::AgentStatusChanged {
            agent_id: next.id,
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        }
    );
    let agent_row = store
        .agents
        .find_one(doc! { "_id": logs[0].agent_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent_row.get_i64("last_seq").unwrap(), 2);
    assert!(agent_row.get_bool("work_pending").unwrap());
    assert_eq!(
        agent_row.get("work_lease_holder"),
        Some(&mongodb::bson::Bson::Null)
    );

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_rejects_an_incorrect_agent_log_high_water_mark() {
    let (client, name) = database("agent_work_incorrect_log_high_water").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "incorrect_log_high_water").await;
    let next = advance_claimed_agent(&claimed);

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: None,
                appended_at: NOW.into(),
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_consumes_only_the_exact_durable_stop_intent() {
    let (client, name) = database("agent_work_stop_intent").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, mut worktree, initial_claimed) =
        claimed_agent_work_fixture(&store, "stop_intent").await;
    let mut scheduled = initial_claimed.clone();
    scheduled
        .request_stop(AgentStopReason::Owner, NOW.into())
        .unwrap();
    store
        .save_agent(initial_claimed.revision, scheduled.clone())
        .await
        .unwrap();
    let claimed = store
        .claim_agent_work(&scheduled.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;

    let mut cleared = claimed.clone();
    cleared
        .work
        .advance("worker", fence, AgentWorkStep::CloseProcess, NOW.into())
        .unwrap();
    cleared.requested_stop_reason = None;
    cleared.revision += 1;
    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence,
                agent: cleared,
                worktree: None,
                appended_at: NOW.into(),
                sandbox_event: None,
                agent_logs: vec![],
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&claimed.id).await.unwrap(),
        Some(claimed.clone())
    );

    let mut claimed = claimed;
    claimed.work.step = AgentWorkStep::PublishStopped;
    claimed.work.cleanup_not_before = None;
    replace_agent_fixture(&store, &claimed).await;
    let mut stopped = claimed.clone();
    stopped
        .work
        .advance("worker", fence, AgentWorkStep::None, MINUTE.into())
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.conversation.last_seq += 1;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Failed).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: stopped.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree,
            }),
            appended_at: MINUTE.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: stopped.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Stopped,
            }),
            agent_logs: vec![AgentLogKind::Stopped {
                reason: AgentStopReason::Owner.as_str().into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(store.get_agent(&stopped.id).await.unwrap(), Some(stopped));

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_invalid_worktree_rolls_back_every_artifact() {
    let (client, name) = database("agent_work_worktree_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, worktree, claimed) = claimed_agent_work_fixture(&store, "worktree_rollback").await;
    let mut next = advance_claimed_agent(&claimed);
    next.conversation.last_seq += 1;
    let mut invalid_worktree = worktree.clone();
    invalid_worktree.status = WorktreeStatus::Ready;
    invalid_worktree.revision += 2;

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: worktree.revision,
                    worktree: invalid_worktree,
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::Created),
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_late_log_conflict_rolls_back_agent_worktree_and_earlier_log() {
    let (client, name) = database("agent_work_log_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "log_rollback").await;
    let conflicting = AgentLogEntry {
        agent_id: claimed.id.clone(),
        seq: 2,
        appended_at: NOW.into(),
        kind: AgentLogKind::Stopped {
            reason: "conflict".into(),
        },
    };
    store
        .agent_log
        .insert_one(log_document(&conflicting).unwrap())
        .await
        .unwrap();
    let mut next = advance_claimed_agent(&claimed);
    next.conversation.last_seq += 2;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: expected_worktree_revision,
                    worktree: worktree.clone(),
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::Created),
                agent_logs: vec![AgentLogKind::Deleted, AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    let mut original_worktree = worktree;
    original_worktree.status = WorktreeStatus::Creating;
    original_worktree.revision = expected_worktree_revision;
    assert_eq!(
        store.get_worktree(&original_worktree.id).await.unwrap(),
        Some(original_worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert!(store
        .agent_log
        .find_one(doc! { "agent_id": conflicting.agent_id.as_str(), "seq": 1_i64 })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_rejects_noncolliding_orphan_log_before_publication() {
    let (client, name) = database("agent_work_orphan_log").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "orphan_log").await;
    let orphan = AgentLogEntry {
        agent_id: claimed.id.clone(),
        seq: 3,
        appended_at: NOW.into(),
        kind: AgentLogKind::Deleted,
    };
    store
        .agent_log
        .insert_one(log_document(&orphan).unwrap())
        .await
        .unwrap();
    let mut next = advance_claimed_agent(&claimed);
    next.conversation.last_seq += 1;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: expected_worktree_revision,
                    worktree: worktree.clone(),
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::Created),
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    let mut original_worktree = worktree;
    original_worktree.status = WorktreeStatus::Creating;
    original_worktree.revision = expected_worktree_revision;
    assert_eq!(
        store.get_worktree(&original_worktree.id).await.unwrap(),
        Some(original_worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert!(store
        .agent_log
        .find_one(doc! { "agent_id": orphan.agent_id.as_str(), "seq": 1_i64 })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_late_event_conflict_rolls_back_agent_worktree_and_logs() {
    let (client, name) = database("agent_work_event_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) =
        claimed_agent_work_fixture(&store, "event_rollback").await;
    let claimed = ready_publication_claim(claimed, "conversation-event-rollback");
    replace_agent_fixture(&store, &claimed).await;
    let conflicting = SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    next.conversation.last_seq += 2;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: expected_worktree_revision,
                    worktree: worktree.clone(),
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                    agent_id: claimed.id.clone(),
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                }),
                agent_logs: vec![
                    AgentLogKind::ConversationOpened {
                        conversation_id: "conversation-event-rollback".into(),
                        runtime_generation: 1,
                    },
                    AgentLogKind::StatusChanged {
                        from: AgentStatus::Starting,
                        to: AgentStatus::Ready,
                    },
                ],
            })
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    let mut original_worktree = worktree;
    original_worktree.status = WorktreeStatus::Creating;
    original_worktree.revision = expected_worktree_revision;
    assert_eq!(
        store.get_worktree(&original_worktree.id).await.unwrap(),
        Some(original_worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        1
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_reopen_reconciliation_requires_every_exact_artifact() {
    let (client, name) = database("agent_work_reconcile").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "reconcile").await;
    let claimed = ready_publication_claim(claimed, "conversation-reconcile");
    replace_agent_fixture(&store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    let log_kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-reconcile".into(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    next.conversation.last_seq += u64::try_from(log_kinds.len()).unwrap();
    let event_kind = SandboxEventKind::AgentStatusChanged {
        agent_id: next.id.clone(),
        from: AgentStatus::Starting,
        to: AgentStatus::Ready,
    };
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: next.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: worktree.clone(),
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(event_kind.clone()),
            agent_logs: log_kinds.clone(),
        })
        .await
        .unwrap();
    let logs = log_kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: next.id.clone(),
            seq: offset as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect::<Vec<_>>();
    let receipt = PublicationReceipt {
        agent: next,
        worktree: Some(worktree),
        sign_in_sandbox: None,
        logs,
        event: Some(SandboxEvent {
            sandbox_id: parent.id.clone(),
            seq: 1,
            appended_at: NOW.into(),
            kind: event_kind,
        }),
    };
    drop(store);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();

    assert!(reconcile_publication(&reopened, &receipt).await.unwrap());
    let mut wrong_agent = receipt.clone();
    wrong_agent.agent.title = Some("wrong".into());
    assert!(!reconcile_publication(&reopened, &wrong_agent)
        .await
        .unwrap());
    let mut wrong_worktree = receipt.clone();
    wrong_worktree.worktree.as_mut().unwrap().branch = "wrong".into();
    assert!(!reconcile_publication(&reopened, &wrong_worktree)
        .await
        .unwrap());
    let mut wrong_log = receipt.clone();
    wrong_log.logs[1].kind = AgentLogKind::Deleted;
    wrong_log.logs[1].appended_at = MINUTE.into();
    assert_eq!(
        reconcile_publication(&reopened, &wrong_log).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    let mut wrong_event = receipt.clone();
    wrong_event.event.as_mut().unwrap().kind = SandboxEventKind::Created;
    assert_eq!(
        reconcile_publication(&reopened, &wrong_event).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    reopened
        .agent_log
        .delete_one(doc! { "agent_id": receipt.agent.id.as_str(), "seq": 2_i64 })
        .await
        .unwrap();
    assert_eq!(
        reconcile_publication(&reopened, &receipt).await,
        Err(SandboxStoreError::SequenceConflict)
    );

    client.database(&name).drop().await.unwrap();
}
mod retired_runtime_tests;
mod sign_in_tests;
