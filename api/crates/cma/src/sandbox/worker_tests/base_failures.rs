use super::*;

#[tokio::test]
async fn install_rejection_fails_with_the_command_index_and_delivers_no_secret() {
    let harness = Harness::new(vec!["false".into()], 0, Duration::from_secs(60)).await;
    harness
        .backend
        .fail_next(WireOperation::Install, "command_failed", "command 0 failed");
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    let failed = harness.stored(&created.id).await;

    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.work.step, SandboxWorkStep::None);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        "terminal_validation"
    );
    assert_eq!(
        failed.failure.as_ref().unwrap().detail,
        "sandbox install command 0 failed"
    );
    assert_terminal_history(&harness, &failed, now).await;
    assert!(harness.actions.snapshot().is_empty());
    assert!(harness
        .backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(request, RequestFrame::Credential(_))));
}

#[tokio::test]
async fn an_ungranted_service_is_a_terminal_typed_failure() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    harness
        .credentials
        .fail_issue(CredentialError::ServiceNotGranted(ServiceId::new(
            "missing-service",
        )));
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    let failed = harness.stored(&created.id).await;

    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        "service_not_granted"
    );
    assert_eq!(failed.failure.as_ref().unwrap().detail, "missing-service");
    assert_terminal_history(&harness, &failed, now).await;
    assert!(harness
        .backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(request, RequestFrame::Credential(_))));
}

#[tokio::test]
async fn transient_failure_keeps_the_step_and_delays_a_fenced_retry() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    harness
        .credentials
        .fail_issue(CredentialError::Unavailable("provider outage".into()));
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    let retrying = harness.stored(&created.id).await;

    assert_eq!(retrying.status, SandboxStatus::Installing);
    assert_eq!(retrying.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(retrying.work.attempts, 1);
    assert!(retrying.work.lease.is_none());
    assert_eq!(
        retrying.work.ready_at,
        rfc3339_at(now + Duration::from_secs(5))
    );
    assert!(!harness.worker.run_one(&created.id, now).await.unwrap());
    harness.run(&created.id, now + Duration::from_secs(5)).await;
    let recovered = harness.stored(&created.id).await;
    assert_eq!(
        recovered.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
    assert_eq!(recovered.work.attempts, 0);

    let events = harness
        .store
        .sandbox_events(&created.id, 0, 20)
        .await
        .unwrap();
    assert!(!events
        .iter()
        .any(|event| matches!(event.kind, SandboxEventKind::Failed { .. })));
    assert!(matches!(events[3].kind, SandboxEventKind::WorkStepFailed {
        step: SandboxWorkStep::MintInitialCredential,
        ref failure,
    } if failure.class == "retryable_transient"));
}

#[tokio::test]
async fn due_sweep_continues_after_one_sandbox_returns_a_store_error() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let first = harness.create().await;
    let second = harness.create().await;
    let (blocked, following) = if first.id < second.id {
        (first, second)
    } else {
        (second, first)
    };
    let now = start_time(&blocked);
    harness.run(&blocked.id, now).await;
    harness.run(&blocked.id, now).await;
    assert_eq!(
        harness.stored(&blocked.id).await.work.step,
        SandboxWorkStep::MintInitialCredential
    );

    let blocking = BlockingCredentials::new(Arc::clone(&harness.credentials));
    let credentials: Arc<dyn SandboxCredentialController> = blocking.clone();
    let worker = harness.worker_with("sweep-worker", credentials, Duration::from_secs(1));
    let entered = blocking.entered.notified();
    let sweep = tokio::spawn(async move { worker.run_due(now).await });
    entered.await;
    harness
        .store
        .claim_sandbox_work(
            &blocked.id,
            "takeover-worker",
            rfc3339_at(now + Duration::from_secs(2)),
            rfc3339_at(now + Duration::from_secs(3)),
        )
        .await
        .unwrap()
        .unwrap();
    blocking.release.notify_one();

    assert_eq!(sweep.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert_eq!(
        harness.stored(&following.id).await.status,
        SandboxStatus::Active
    );
}

async fn assert_terminal_history(harness: &Harness, failed: &Sandbox, now: SystemTime) {
    let events = harness
        .store
        .sandbox_events(&failed.id, 0, 200)
        .await
        .unwrap();
    let mut status = SandboxStatus::Provisioning;
    let mut terminal_count = 0;
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.seq, index as u64 + 1);
        match &event.kind {
            SandboxEventKind::StatusChanged { to, .. } => status = *to,
            SandboxEventKind::Failed { failure } => {
                status = SandboxStatus::Failed;
                terminal_count += 1;
                assert_eq!(Some(failure), failed.failure.as_ref());
                assert_eq!(event.appended_at, failed.updated_at);
            }
            _ => {}
        }
    }
    assert_eq!(terminal_count, 1);
    assert_eq!(status, failed.status);
    assert!(!harness.worker.run_one(&failed.id, now).await.unwrap());
    assert_eq!(
        harness
            .store
            .sandbox_events(&failed.id, 0, 200)
            .await
            .unwrap(),
        events
    );
}
