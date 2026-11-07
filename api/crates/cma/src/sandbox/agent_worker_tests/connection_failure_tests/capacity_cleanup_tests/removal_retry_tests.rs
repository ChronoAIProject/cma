use super::*;

async fn pending(h: &Harness<MongoSandboxStore>) -> Agent {
    let agent = h.admit("removal-retry", vec![]).await;
    for _ in 0..5 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let ready = h.stored(&agent.id).await;
    h.store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let stopped = h.stored(&agent.id).await;
    h.store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    for _ in 0..2 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let pending = h.stored(&agent.id).await;
    assert_eq!(pending.work.step, AgentWorkStep::RemoveWorktree);
    pending
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn removal_failure_backoff_survives_restart_and_success_finishes_once() {
    removal_retry_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn full_history_removal_keeps_backoff_in_capacity_settlement() {
    removal_retry_case(true).await;
}

async fn removal_retry_case(full: bool) {
    let (client, name, h) = fixture().await;
    let initial = pending(&h).await;
    if full {
        fill_history(&client, &name, &h, &initial.id).await;
    }
    let mut now = SystemTime::now();
    for delay in [30, 60, 120, 240, 300, 300] {
        let started_at = layer2_core::seconds_from_rfc3339(&rfc3339_at(now)).unwrap();
        h.backend.fail_next(
            WireOperation::RemoveWorktree,
            "mutation_authority_corrupt",
            "synthetic authority rejection",
        );
        h.run(&initial.id, now).await;
        let current = h.stored(&initial.id).await;
        let due = layer2_core::seconds_from_rfc3339(&current.work.ready_at).unwrap();
        let observed_at = layer2_core::seconds_from_rfc3339(&current.updated_at).unwrap();
        assert!(observed_at >= started_at);
        assert_eq!(due - observed_at, delay);
        assert_eq!(current.work.step, AgentWorkStep::RemoveWorktree);
        assert_eq!(current.status, AgentStatus::Stopped);
        assert_eq!(
            current.conversation.conversation_id,
            initial.conversation.conversation_id
        );
        let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let replacement = AgentWorker::new(
            AgentWorkerDependencies {
                store: reopened.clone(),
                adapter: h.adapter.clone(),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: h.registry.clone(),
                updates: h.updates.clone(),
                operations: AgentOperationService::new(reopened, h.registry.clone()),
            },
            AgentWorkerConfig::new("replacement", DEPLOYMENT_ID),
        );
        assert!(!replacement
            .run_one(
                &initial.id,
                SystemTime::UNIX_EPOCH + Duration::from_secs(due - 1)
            )
            .await
            .unwrap());
        now = SystemTime::UNIX_EPOCH + Duration::from_secs(due);
    }
    // A lost acknowledgement leaves the effect uncertain, without a receipt or
    // false completion. The next attempt must reconcile the original operation.
    h.backend.lose_next_response(WireOperation::RemoveWorktree);
    h.run(&initial.id, now).await;
    let uncertain = h.stored(&initial.id).await;
    assert_eq!(uncertain.work.step, AgentWorkStep::RemoveWorktree);
    assert!(!uncertain
        .work
        .receipts
        .iter()
        .any(|r| r.step == AgentWorkStep::RemoveWorktree));
    now = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&uncertain.work.ready_at).unwrap());
    // A successful retry keeps the original operation receipt and resets pacing
    // when it advances. A second observer cannot repeat terminal cleanup.
    h.run(&initial.id, now).await;
    let removed = h.stored(&initial.id).await;
    assert_eq!(removed.work.step, AgentWorkStep::PublishDeleted);
    assert_eq!(removed.work.attempts, 0);
    h.run(&initial.id, now + Duration::from_secs(1)).await;
    let deleted = h.stored(&initial.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert!(!h
        .worker
        .run_one(&initial.id, now + Duration::from_secs(2))
        .await
        .unwrap());
    assert_eq!(
        deleted
            .work
            .receipts
            .iter()
            .filter(|r| r.step == AgentWorkStep::RemoveWorktree)
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn new_holder_retries_at_deadline_without_resetting_attempts() {
    let (client, name, h) = fixture().await;
    let initial = pending(&h).await;
    let mut now = SystemTime::now();
    for (attempt, expected_delay) in [30_u64, 60, 120, 240, 300].into_iter().enumerate() {
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let replacement = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter: h.adapter.clone(),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: h.registry.clone(),
                updates: h.updates.clone(),
                operations: AgentOperationService::new(store, h.registry.clone()),
            },
            AgentWorkerConfig::new(format!("takeover-holder-{attempt}"), DEPLOYMENT_ID),
        );
        h.backend.fail_next(
            WireOperation::RemoveWorktree,
            "mutation_authority_corrupt",
            "synthetic authority rejection",
        );
        assert!(replacement.run_one(&initial.id, now).await.unwrap());
        let current = h.stored(&initial.id).await;
        assert_eq!(current.work.attempts, attempt as u32 + 1);
        let due = layer2_core::seconds_from_rfc3339(&current.work.ready_at).unwrap();
        assert_eq!(
            due - layer2_core::seconds_from_rfc3339(&current.updated_at).unwrap(),
            expected_delay
        );
        assert!(current.work.lease.is_none());
        assert!(!replacement
            .run_one(
                &initial.id,
                SystemTime::UNIX_EPOCH + Duration::from_secs(due - 1)
            )
            .await
            .unwrap());
        now = SystemTime::UNIX_EPOCH + Duration::from_secs(due);
    }
    h.run(&initial.id, now).await;
    assert_eq!(h.stored(&initial.id).await.work.attempts, 0);
    assert_eq!(
        h.stored(&initial.id).await.work.step,
        AgentWorkStep::PublishDeleted
    );
    client.database(&name).drop().await.unwrap();
}
