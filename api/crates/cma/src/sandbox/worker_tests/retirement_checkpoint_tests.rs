use super::*;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::{AgentLogKind, AgentWorkStep};

#[tokio::test]
async fn retirement_retries_snapshot_cleanup_across_restart_without_removing_history() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(3600)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let agent = lifecycle_queue_tests::ready_agent(&active, now);
    harness.store.create_agent(agent.clone()).await.unwrap();
    harness
        .store
        .append_agent_log(
            &agent.id,
            rfc3339_at(now),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            },
        )
        .await
        .unwrap();
    let history = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    harness.checkpoint_failures.store(1, Ordering::SeqCst);
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, now).await;
    }
    let retrying = harness.stored(&active.id).await;
    assert_eq!(retrying.status, SandboxStatus::Retiring);
    assert_eq!(retrying.work.step, SandboxWorkStep::MarkRetired);
    assert!(retrying.runtime.is_none());
    assert!(retrying.purge_after.is_none());
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        history
    );
    assert!(harness.store.get_agent(&agent.id).await.unwrap().is_some());
    assert!(harness
        .store
        .sandbox_events(&active.id, 0, 100)
        .await
        .unwrap()
        .iter()
        .any(|event| matches!(
            event.kind,
            SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::MarkRetired,
                ..
            }
        )));

    let worker = harness.worker_with(
        "retirement-restarted",
        harness.credentials.clone(),
        Duration::from_secs(60),
    );
    let retry_at = now + Duration::from_secs(10);
    assert!(worker.run_one(&active.id, retry_at).await.unwrap());
    let retired = harness.stored(&active.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert_eq!(retired.work.step, SandboxWorkStep::WaitForPurge);
    assert_eq!(
        retired.purge_after,
        Some(rfc3339_at(retry_at + Duration::from_secs(3600)))
    );
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        history
    );
    assert!(harness.store.get_agent(&agent.id).await.unwrap().is_some());
    assert_eq!(
        harness
            .actions
            .snapshot()
            .iter()
            .filter(|action| action.as_str() == "purge-checkpoints")
            .count(),
        2
    );
}
