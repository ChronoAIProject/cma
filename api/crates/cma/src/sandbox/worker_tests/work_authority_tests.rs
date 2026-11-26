use super::*;
use crate::sandbox::workspace_worker::WorkspaceWorkerConfig;
use layer2_core::sandbox::store::{WorkAuthorityProtocol, WORK_LEASE_SECONDS, WORK_RENEW_SECONDS};

mod workspace;

#[test]
fn production_builders_separate_authority_duration_from_effect_timeout() {
    let sandbox =
        || SandboxWorkerConfig::new("worker", "test", runtime_image(), SandboxPolicy::default());
    let workspace = || WorkspaceWorkerConfig::new("worker", "test");
    assert_eq!(sandbox().lease_duration, Duration::from_secs(900));
    assert_eq!(workspace().lease_duration, Duration::from_secs(900));
    assert_eq!(
        sandbox()
            .with_execution(Default::default())
            .with_replicas(false)
            .lease_duration,
        Duration::from_secs(1860)
    );
    assert_eq!(
        workspace()
            .with_execution(Default::default())
            .with_replicas(false)
            .lease_duration,
        Duration::from_secs(1860)
    );
    for duration in [
        sandbox()
            .with_execution(Default::default())
            .with_replicas(true)
            .lease_duration,
        sandbox()
            .with_replicas(true)
            .with_execution(Default::default())
            .lease_duration,
        workspace()
            .with_execution(Default::default())
            .with_replicas(true)
            .lease_duration,
        workspace()
            .with_replicas(true)
            .with_execution(Default::default())
            .lease_duration,
    ] {
        assert_eq!(duration, Duration::from_secs(WORK_LEASE_SECONDS));
        assert!(duration <= Duration::from_secs(15));
    }
    const { assert!(WORK_RENEW_SECONDS <= 3) };
}

#[tokio::test(start_paused = true)]
async fn composed_sandbox_worker_renews_while_credential_future_is_pending() {
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let now = start_time(&created);
    h.run(&created.id, now).await;
    h.run(&created.id, now).await;
    let mut protected = h.stored(&created.id).await;
    let revision = protected.revision;
    protected.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
    protected.revision += 1;
    h.store.save_sandbox(revision, protected).await.unwrap();
    let blocked = BlockingCredentials::new(h.credentials.clone());
    let worker = renewable(&h, blocked.clone());
    let entered = blocked.entered.notified();
    let id = created.id.clone();
    let running = tokio::spawn(async move { worker.run_one(&id, now).await });
    entered.await;
    let initial = h.stored(&created.id).await.work.lease.unwrap();
    for _ in 0..7 {
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
    }
    let extended = h.stored(&created.id).await.work.lease.unwrap();
    assert_eq!(extended.holder, initial.holder);
    assert_eq!(extended.fence, initial.fence);
    assert!(extended.expires_at > rfc3339_at(now + Duration::from_secs(21)));
    assert!(h
        .store
        .claim_sandbox_work(
            &created.id,
            "survivor",
            rfc3339_at(now + Duration::from_secs(21)),
            rfc3339_at(now + Duration::from_secs(36))
        )
        .await
        .unwrap()
        .is_none());
    blocked.release.notify_one();
    assert_eq!(running.await.unwrap(), Ok(true));
    let finished = h.stored(&created.id).await;
    assert_eq!(
        finished.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
    assert!(finished.work.lease.is_none());
    assert_eq!(h.actions.snapshot(), ["mint:1"]);
}

#[tokio::test(start_paused = true)]
async fn two_instance_owner_abort_is_taken_over_after_fifteen_seconds_once() {
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (created, now) = prepare_renewable_sandbox(&h).await;

    let owner_credentials = BlockingCredentials::new(h.credentials.clone());
    let owner = renewable_for(&h, "owner-a", owner_credentials.clone());
    let successor = renewable_for(&h, "owner-b", h.credentials.clone());
    let id = created.id.clone();
    let owner_task = tokio::spawn({
        let owner = owner.clone();
        async move { owner.run_one(&id, now).await }
    });
    owner_credentials.entered.notified().await;

    // The owner is terminated while its credential effect is still pending.
    // Aborting the task models process loss and drops its local authority.
    let fault_at = tokio::time::Instant::now();
    owner_task.abort();
    assert!(owner_task.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(WORK_LEASE_SECONDS)).await;
    tokio::task::yield_now().await;
    let takeover = successor.run_one(&created.id, now + Duration::from_secs(WORK_LEASE_SECONDS));
    assert!(takeover.await.unwrap());
    let takeover_at = tokio::time::Instant::now();

    let takeover_time = takeover_at - fault_at;
    assert!(takeover_time <= Duration::from_secs(WORK_LEASE_SECONDS));
    let finished = h.stored(&created.id).await;
    assert!(finished.work.lease.is_none());
    assert_eq!(h.actions.snapshot(), ["mint:1"]);
}

async fn prepare_renewable_sandbox(h: &Harness) -> (Sandbox, SystemTime) {
    let created = h.create().await;
    let now = start_time(&created);
    h.run(&created.id, now).await;
    h.run(&created.id, now).await;
    let mut protected = h.stored(&created.id).await;
    let revision = protected.revision;
    protected.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
    protected.revision += 1;
    h.store.save_sandbox(revision, protected).await.unwrap();
    (created, now)
}

fn renewable(h: &Harness, credentials: Arc<dyn SandboxCredentialController>) -> Arc<SandboxWorker> {
    renewable_for(h, "renewed-worker", credentials)
}

fn renewable_for(
    h: &Harness,
    holder: &str,
    credentials: Arc<dyn SandboxCredentialController>,
) -> Arc<SandboxWorker> {
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
            credentials,
            workspaces: Arc::new(WorkspaceRecorder {
                actions: h.actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: h.actions.clone(),
            }),
            registry: h.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(h.store.clone()),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        SandboxWorkerConfig::new(holder, "test", runtime_image(), h.policy.clone())
            .with_execution(Default::default())
            .with_replicas(true),
    )
}

#[tokio::test]
async fn diagnostic_claim_refusal_keeps_the_original_error_without_effects() {
    use crate::sandbox::lifecycle_failure::{CauseClass, Origin};
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let worker = renewable_for(&h, "", h.credentials.clone());
    let failure = worker
        .run_one_diagnostic(&created.id, start_time(&created))
        .await
        .unwrap_err();
    assert_eq!(failure.observation.origin, Origin::Claim);
    assert_eq!(
        failure.observation.cause_class,
        CauseClass::StoredDataInvalid
    );
    assert!(matches!(
        failure.into_compatibility(),
        SandboxStoreError::Corrupt(_)
    ));
    assert_eq!(h.stored(&created.id).await, created);
    assert!(h.actions.snapshot().is_empty());
}

#[tokio::test(start_paused = true)]
async fn diagnostic_renewal_conflict_keeps_execution_origin_and_stale_worker_fenced() {
    use crate::sandbox::lifecycle_failure::{CauseClass, Origin};
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (created, now) = prepare_renewable_sandbox(&h).await;
    let blocked = BlockingCredentials::new(h.credentials.clone());
    let worker = renewable(&h, blocked.clone());
    let id = created.id.clone();
    let running = tokio::spawn(async move { worker.run_one_diagnostic(&id, now).await });
    blocked.entered.notified().await;
    let mut current = h.stored(&created.id).await;
    let revision = current.revision;
    current.work.lease.as_mut().unwrap().holder = "successor".into();
    current.revision += 1;
    h.store
        .save_sandbox(revision, current.clone())
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(WORK_RENEW_SECONDS)).await;
    let failure = running.await.unwrap().unwrap_err();
    assert_eq!(failure.observation.origin, Origin::Execute);
    assert_eq!(failure.observation.cause_class, CauseClass::StoreConflict);
    assert_eq!(failure.into_compatibility(), SandboxStoreError::Conflict);
    assert_eq!(h.stored(&created.id).await, current);
    assert!(h.actions.snapshot().is_empty());
}
