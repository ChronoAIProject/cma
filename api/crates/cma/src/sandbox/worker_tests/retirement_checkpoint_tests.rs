use super::*;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::{AgentLogKind, AgentWorkStep};

#[tokio::test]
async fn missing_retiring_runtime_settles_real_agents_without_archive_cleanup() {
    use crate::sandbox::checkpoint_maintenance::SandboxCheckpointMaintenance;
    use crate::sandbox::live_agent_stopper::DurableLiveAgentStopper;
    use crate::sandbox::reconciliation::SandboxReconciler;
    use layer2_core::sandbox::{AgentStatus, AgentStopReason, SANDBOX_CHECKPOINT_INTERVAL_SECONDS};

    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(3600)).await;
    let created = harness.create().await;
    let started = start_time(&created);
    let active = harness.drive_active(&created.id, started).await;
    let now = started + Duration::from_secs(SANDBOX_CHECKPOINT_INTERVAL_SECONDS + 1);
    let maintenance = SandboxCheckpointMaintenance::new(
        harness.store.clone(),
        harness.adapter.clone(),
        "test".into(),
        "capture".into(),
    );
    maintenance.run_once(now).await.unwrap();
    maintenance.run_once(now).await.unwrap();
    let published = harness.stored(&active.id).await;
    assert!(published.lifecycle.checkpoints.latest.is_some());
    assert!(published.lifecycle.checkpoint_capture.is_some());
    // The checkpoint fixture advances its synthetic clock to make capture due;
    // the Agent and retirement observation must use a real boundary.
    let retirement_at = SystemTime::now();
    let mut agent = lifecycle_queue_tests::ready_agent(&active, retirement_at);
    agent.conversation.last_seq = 1;
    harness.store.create_agent(agent.clone()).await.unwrap();
    harness
        .store
        .append_agent_log(
            &agent.id,
            rfc3339_at(retirement_at),
            AgentLogKind::OwnerMessage {
                content: "retained owner history".into(),
            },
        )
        .await
        .unwrap();
    let history = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    // Retirement itself must use a real observation boundary so a provider
    // absence read is never compared with a future durable timestamp.
    let mut retiring = published.clone();
    retiring
        .begin_retirement(rfc3339_at(retirement_at))
        .unwrap();
    harness
        .store
        .save_sandbox(published.revision, retiring.clone())
        .await
        .unwrap();

    // An empty stale scan followed by the actual runtime must preserve authority.
    let stale = Arc::new(CodexAdapter::with_backend(
        super::reconciliation_tests::InventoryProbe::new(
            harness.backend.clone(),
            vec![Ok(Vec::new())],
        ),
        CodexSandboxSpec::default(),
    ));
    SandboxReconciler::new(
        harness.store.clone(),
        stale,
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(retirement_at)
    .await
    .unwrap();
    assert_eq!(harness.stored(&active.id).await, retiring);
    assert_eq!(
        harness.store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );

    let runtime = RuntimeHandle(retiring.runtime.as_ref().unwrap().runtime_id.clone());
    harness.adapter.teardown(&runtime).await.unwrap();
    // The startup inventory itself is global evidence: its failure still stops startup.
    let global = Arc::new(CodexAdapter::with_backend(
        super::reconciliation_tests::InventoryProbe::new(
            harness.backend.clone(),
            vec![Err(layer3_adapter::AdapterError::Unavailable(
                "inventory unavailable".into(),
            ))],
        ),
        CodexSandboxSpec::default(),
    ));
    assert!(SandboxReconciler::new(
        harness.store.clone(),
        global,
        Arc::new(harness.policy.clone()),
        "test".into()
    )
    .initialize(retirement_at)
    .await
    .is_err());
    assert_eq!(harness.stored(&active.id).await, retiring);
    // A per-Sandbox final loss observation that is unavailable defers only this
    // binding (#3187). Without fresh absence evidence nothing is settled.
    let unavailable = Arc::new(CodexAdapter::with_backend(
        super::reconciliation_tests::InventoryProbe::new(
            harness.backend.clone(),
            vec![
                Ok(Vec::new()),
                Err(layer3_adapter::AdapterError::Unavailable(
                    "inventory unavailable".into(),
                )),
            ],
        ),
        CodexSandboxSpec::default(),
    ));
    SandboxReconciler::new(
        harness.store.clone(),
        unavailable,
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(retirement_at)
    .await
    .unwrap();
    assert_eq!(harness.stored(&active.id).await, retiring);
    assert_eq!(
        harness.store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );

    SandboxReconciler::new(
        harness.store.clone(),
        harness.adapter.clone(),
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(retirement_at)
    .await
    .unwrap();
    let reconciled = harness.stored(&active.id).await;
    assert_eq!(reconciled.status, SandboxStatus::Retiring);
    assert!(reconciled.runtime.is_none());
    assert!(reconciled.lifecycle.checkpoint_capture.is_none());
    assert_eq!(
        reconciled.lifecycle.checkpoints,
        published.lifecycle.checkpoints
    );
    let stopped = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.stop_reason, Some(AgentStopReason::SandboxRetired));
    assert_eq!(
        stopped.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(&logs[..history.len()], history.as_slice());

    // Exercise the production stopper, not the harness's unconditional recorder.
    let worker = SandboxWorker::new(
        SandboxWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            credentials: harness.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: harness.actions.clone(),
            }),
            agents: Arc::new(DurableLiveAgentStopper::new(
                harness.store.clone(),
                harness.registry.clone(),
            )),
            registry: harness.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        SandboxWorkerConfig::new(
            "retirement",
            "test",
            runtime_image(),
            harness.policy.clone(),
        ),
    );
    for _ in 0..3 {
        worker.run_one(&active.id, now).await.unwrap();
    }
    let retrying = harness.stored(&active.id).await;
    assert_eq!(retrying.status, SandboxStatus::Retiring);
    assert_eq!(retrying.work.step, SandboxWorkStep::MarkRetired);
    assert!(retrying.credential.current.is_none());
    worker
        .run_one(&active.id, now + Duration::from_secs(60))
        .await
        .unwrap();
    let retired = harness.stored(&active.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert_eq!(retired.work.step, SandboxWorkStep::WaitForPurge);
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        logs
    );
    assert!(harness
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    assert!(!harness
        .actions
        .snapshot()
        .iter()
        .any(|action| action.as_str() == "purge-checkpoints"));
}

#[tokio::test]
async fn retirement_across_restart_preserves_history_without_snapshot_cleanup() {
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
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    for _ in 0..3 {
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
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::TeardownRuntime,
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
    assert!(!harness
        .actions
        .snapshot()
        .iter()
        .any(|action| action.as_str() == "purge-checkpoints"));
}
