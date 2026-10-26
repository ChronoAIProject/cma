use super::*;

async fn failed_revive() -> (Fixture, Sandbox, layer2_core::sandbox::Agent, SystemTime) {
    let mut fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let mut agent =
        lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    let launch = fixture
        .backend
        .snapshot()
        .await
        .agents
        .into_values()
        .next()
        .unwrap();
    agent
        .work
        .receipts
        .push(layer2_core::sandbox::AgentWorkReceipt {
            step: layer2_core::sandbox::AgentWorkStep::OpenConversation,
            operation_id: launch.operation_id,
            request_sha256: launch.request_sha256,
            completed_at: rfc3339_at(now),
        });
    let revision = agent.revision;
    agent.revision += 1;
    fixture
        .harness
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();
    fixture
        .harness
        .store
        .create_workspace(lifecycle_recovery_tests::workspace_for(&agent, now))
        .await
        .unwrap();
    let paused = fixture.pause(&active, &mut now).await;
    fixture
        .harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(now),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    fixture
        .backend
        .reject_credential
        .store(true, Ordering::SeqCst);
    let failed = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Failed,
            &runtime_image(),
        )
        .await;
    assert!(failed.runtime.is_some());
    assert!(failed.lifecycle.suspended_runtime.is_none());
    // This durable flag records the old process's credential delivery.
    assert!(failed.credential.delivered);
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    (fixture, failed, agent, now)
}

#[tokio::test]
async fn failed_native_credential_delivery_retires_without_recredentialing_or_closing_old_agents() {
    let (fixture, failed, agent, mut now) = failed_revive().await;
    let requests_before = fixture.harness.backend.requests().await.len();
    fixture.backend.lose_delete.store(true, Ordering::SeqCst);
    let mut retiring = failed.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    fixture
        .harness
        .store
        .save_sandbox(failed.revision, retiring)
        .await
        .unwrap();
    let retired = fixture
        .drive(
            &failed.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.runtime.is_none());
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(fixture
        .adapter(&runtime_image())
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    let stopped = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.work.receipts, agent.work.receipts);
    assert_eq!(
        stopped.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    let requests = fixture.harness.backend.requests().await;
    assert!(requests[requests_before..]
        .iter()
        .all(|(_, request)| !matches!(
            request,
            RequestFrame::Credential(_) | RequestFrame::CloseAgent(_) | RequestFrame::OpenAgent(_)
        )));
    assert_eq!(
        fixture
            .backend
            .calls
            .snapshot()
            .iter()
            .filter(|call| *call == "create")
            .count(),
        1
    );
    assert_eq!(
        fixture
            .backend
            .calls
            .snapshot()
            .iter()
            .filter(|call| *call == "delete-retained")
            .count(),
        2
    );
}

#[tokio::test]
async fn retiring_native_runtime_requires_exact_idle_uncredentialed_process_evidence() {
    for changed in [
        "credential",
        "process",
        "boot",
        "generation",
        "home",
        "sandbox",
    ] {
        let (fixture, failed, _, now) = failed_revive().await;
        let adapter = fixture.adapter(&runtime_image());
        let handle = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            adapter.as_ref(),
            &failed,
            "test",
        )
        .await
        .unwrap();
        let mut status = adapter.runtime_status(&handle).await.unwrap();
        assert!(!status.credential_ready);
        assert_eq!(status.agent_processes, 0);
        match changed {
            "credential" => status.credential_ready = true,
            "process" => status.agent_processes = 1,
            "boot" => status.boot_id = "f".repeat(32),
            "generation" => status.generation += 1,
            "home" => status.home_identity = "x".repeat(43),
            "sandbox" => status.sandbox_id = "another-sandbox".into(),
            _ => unreachable!(),
        }
        *fixture.backend.status_override.lock().unwrap() = Some(status);
        let mut retiring = failed.clone();
        retiring.begin_retirement(rfc3339_at(now)).unwrap();
        fixture
            .harness
            .store
            .save_sandbox(failed.revision, retiring)
            .await
            .unwrap();
        let worker = fixture
            .harness
            .worker_with_adapter("retirement-guard", adapter);
        assert!(worker.run_one(&failed.id, now).await.unwrap());
        let pending = fixture.harness.stored(&failed.id).await;
        assert_eq!(pending.runtime, failed.runtime, "{changed}");
        assert!(
            !pending.lifecycle.home.as_ref().unwrap().released,
            "{changed}"
        );
        assert!(
            !fixture
                .backend
                .calls
                .snapshot()
                .contains(&"delete-retained".into()),
            "{changed}"
        );
        assert_eq!(
            pending.work.step,
            if changed == "credential" {
                // The fixture stopper completes immediately on the graceful path.
                SandboxWorkStep::RevokeRetirementCredentials
            } else {
                SandboxWorkStep::StopAgents
            },
            "{changed}"
        );
    }
}
