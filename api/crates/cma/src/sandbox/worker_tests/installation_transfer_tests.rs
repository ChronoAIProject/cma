//! Exercise the production adoption/publication path with a prepared hierarchy.
//! The Mongo tests separately prove that only the exact owner obtains its digest.
use super::*;

async fn owned(harness: &Harness) -> (Arc<Pools>, Sandbox, String, SystemTime) {
    let pools = pools(harness, &[]);
    let prepared = ready(harness, &pools, reservation(harness).await).await;
    let mut sandbox = harness.stored(&prepared.id).await;
    let revision = sandbox.revision;
    sandbox.profile = prepared.plan.profile.clone();
    sandbox.runtime = prepared.runtime.clone();
    sandbox.lifecycle.home = prepared.home.clone();
    sandbox.lifecycle.runtime_generation_high_water = 1;
    sandbox.work.execution = Some(prepared.plan.execution);
    sandbox.revision += 1;
    harness
        .store
        .save_sandbox(revision, sandbox.clone())
        .await
        .unwrap();
    let now = start_time(&sandbox) + Duration::from_secs(1);
    (
        pools,
        sandbox,
        prepared.installed_request_sha256.unwrap(),
        now,
    )
}

async fn claim(harness: &Harness, sandbox: &Sandbox, holder: &str, now: SystemTime) -> Sandbox {
    harness
        .store
        .claim_sandbox_work(
            &sandbox.id,
            holder,
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn installation_transfer_survives_worker_restart_and_checks_again_before_delivery() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, sandbox, digest, now) = owned(&harness).await;
    let original = sandbox.runtime.clone();
    let worker = harness.worker_with_adapter("transfer", adapter(&harness, pools.clone()));
    let claimed = claim(&harness, &sandbox, "transfer", now).await;
    let before = harness.backend.requests().await.len();
    assert!(worker
        .adopt_initial_runtime(claimed, Some(&digest), now)
        .await
        .unwrap()
        .is_none());
    let installed = harness.stored(&sandbox.id).await;
    assert_eq!(installed.status, SandboxStatus::Installing);
    assert_eq!(installed.work.step, SandboxWorkStep::MintInitialCredential);
    assert_eq!(installed.runtime, original);
    assert!(harness.actions.snapshot().is_empty());
    let requests = harness.backend.requests().await;
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|(_, r)| matches!(r, RequestFrame::RuntimeStatus(_)))
            .count(),
        1
    );
    assert!(!requests[before..]
        .iter()
        .any(|(_, r)| matches!(r, RequestFrame::Install(_) | RequestFrame::Credential(_))));
    // Only the existing owned schema and work step survive this replica change.
    let persisted: Sandbox =
        serde_json::from_slice(&serde_json::to_vec(&installed).unwrap()).unwrap();
    assert_eq!(persisted, installed);
    drop(worker);
    let restarted = harness.worker_with_adapter("restarted", adapter(&harness, pools));
    assert!(restarted.run_one(&sandbox.id, now).await.unwrap());
    assert_eq!(
        harness.stored(&sandbox.id).await.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
    assert_eq!(harness.actions.snapshot(), ["mint:1"]);
    let before_delivery = harness.backend.requests().await.len();
    assert!(restarted.run_one(&sandbox.id, now).await.unwrap());
    assert!(harness.stored(&sandbox.id).await.credential.delivered);
    let requests = harness.backend.requests().await;
    assert_eq!(
        requests[before_delivery..]
            .iter()
            .filter(|(_, r)| matches!(r, RequestFrame::RuntimeStatus(_)))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|(_, r)| matches!(r, RequestFrame::Install(_)))
            .count(),
        1,
        "only the original preparation installs"
    );
}

#[tokio::test]
async fn installation_transfer_mismatched_digest_or_execution_replays_installation() {
    for mismatch in 0..3 {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let (pools, sandbox, mut digest, now) = owned(&harness).await;
        if mismatch == 1 {
            let execution = sandbox.work.execution.unwrap();
            digest = layer3_adapter::InstallRequest::with_timeouts(
                format!("sandbox-install-v1:{}", sandbox.id),
                sandbox.profile.content.software_install_commands.clone(),
                execution.command_timeout_seconds + 1,
                execution.total_timeout_seconds,
                layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
            )
            .request_sha256;
        }
        let worker = harness.worker_with_adapter("transfer", adapter(&harness, pools));
        let claimed = claim(&harness, &sandbox, "transfer", now).await;
        let bad = "a".repeat(64);
        let proof = match mismatch {
            0 => Some(bad.as_str()),
            1 => Some(digest.as_str()),
            _ => None,
        };
        worker
            .adopt_initial_runtime(claimed, proof, now)
            .await
            .unwrap();
        assert_eq!(
            harness.stored(&sandbox.id).await.work.step,
            SandboxWorkStep::RunInstall
        );
        assert!(harness.actions.snapshot().is_empty());
        if mismatch != 1 {
            assert!(worker.run_one(&sandbox.id, now).await.unwrap());
            assert_eq!(
                harness.stored(&sandbox.id).await.work.step,
                SandboxWorkStep::MintInitialCredential
            );
            assert_eq!(
                harness
                    .backend
                    .requests()
                    .await
                    .iter()
                    .filter(|(_, r)| matches!(r, RequestFrame::Install(_)))
                    .count(),
                2
            );
        }
    }
}

#[tokio::test]
async fn installation_transfer_missing_runtime_reserves_replacement_before_any_credential() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, sandbox, digest, now) = owned(&harness).await;
    let runtime_adapter = adapter(&harness, pools);
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        runtime_adapter.as_ref(),
        &sandbox,
        "test",
    )
    .await
    .unwrap();
    let handle = RuntimeHandle::new(&sandbox.runtime.as_ref().unwrap().runtime_id);
    runtime_adapter.teardown(&handle).await.unwrap();
    let worker = harness.worker_with_adapter("transfer", runtime_adapter);
    let claimed = claim(&harness, &sandbox, "transfer", now).await;
    let replacement = worker
        .adopt_initial_runtime(claimed, Some(&digest), now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(harness.stored(&sandbox.id).await, replacement);
    assert_eq!(replacement.work.step, SandboxWorkStep::ProvisionRuntime);
    assert!(replacement.runtime.is_none());
    assert_eq!(replacement.lifecycle.runtime_generation_high_water, 2);
    assert_eq!(replacement.lifecycle.home, sandbox.lifecycle.home);
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn installation_transfer_replaced_boot_waits_for_durable_recovery() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, sandbox, digest, now) = owned(&harness).await;
    let handle = RuntimeHandle::new(&sandbox.runtime.as_ref().unwrap().runtime_id);
    harness
        .backend
        .restart_persistent_process(&handle)
        .await
        .unwrap();
    let worker = harness.worker_with_adapter("transfer", adapter(&harness, pools));
    let claimed = claim(&harness, &sandbox, "transfer", now).await;
    worker
        .adopt_initial_runtime(claimed, Some(&digest), now)
        .await
        .unwrap();
    let retry = harness.stored(&sandbox.id).await;
    assert_eq!(retry.status, SandboxStatus::Provisioning);
    assert_eq!(retry.work.step, SandboxWorkStep::ProvisionRuntime);
    assert_eq!(retry.runtime, sandbox.runtime);
    assert!(retry.failure.is_none());
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn installation_transfer_lost_inspection_is_retryable_without_minting() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, sandbox, digest, now) = owned(&harness).await;
    harness
        .backend
        .lose_next_response(WireOperation::RuntimeStatus);
    let worker = harness.worker_with_adapter("transfer", adapter(&harness, pools.clone()));
    let claimed = claim(&harness, &sandbox, "transfer", now).await;
    worker
        .adopt_initial_runtime(claimed, Some(&digest), now)
        .await
        .unwrap();
    let retry = harness.stored(&sandbox.id).await;
    assert_eq!(retry.work.step, SandboxWorkStep::ProvisionRuntime);
    assert!(harness.actions.snapshot().is_empty());
    let later = now + Duration::from_secs(10);
    let restarted = harness.worker_with_adapter("restarted", adapter(&harness, pools));
    let claimed = claim(&harness, &retry, "restarted", later).await;
    restarted
        .adopt_initial_runtime(claimed, Some(&digest), later)
        .await
        .unwrap();
    assert_eq!(
        harness.stored(&sandbox.id).await.work.step,
        SandboxWorkStep::MintInitialCredential
    );
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn installation_transfer_stale_worker_cannot_publish_after_takeover() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, sandbox, digest, now) = owned(&harness).await;
    let worker = harness.worker_with_adapter("stale", adapter(&harness, pools));
    let stale = claim(&harness, &sandbox, "stale", now).await;
    let current = claim(
        &harness,
        &sandbox,
        "takeover",
        now + Duration::from_secs(61),
    )
    .await;
    assert!(matches!(
        worker
            .adopt_initial_runtime(stale, Some(&digest), now)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(harness.stored(&sandbox.id).await, current);
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn installation_transfer_does_not_bypass_delivery_boot_check() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, sandbox, digest, now) = owned(&harness).await;
    let worker = harness.worker_with_adapter("transfer", adapter(&harness, pools));
    let claimed = claim(&harness, &sandbox, "transfer", now).await;
    worker
        .adopt_initial_runtime(claimed, Some(&digest), now)
        .await
        .unwrap();
    assert!(worker.run_one(&sandbox.id, now).await.unwrap());
    let handle = RuntimeHandle::new(&sandbox.runtime.as_ref().unwrap().runtime_id);
    harness
        .backend
        .restart_persistent_process(&handle)
        .await
        .unwrap();
    assert!(worker.run_one(&sandbox.id, now).await.unwrap());
    let retry = harness.stored(&sandbox.id).await;
    assert_eq!(retry.work.step, SandboxWorkStep::DeliverInitialCredential);
    assert!(!retry.credential.delivered);
    assert!(
        !harness
            .backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
}

#[tokio::test]
async fn installation_transfer_refuses_unexpected_runtime_credential() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, sandbox, digest, now) = owned(&harness).await;
    let runtime_adapter = adapter(&harness, pools);
    let handle = RuntimeHandle::new(&sandbox.runtime.as_ref().unwrap().runtime_id);
    let worker = harness.worker_with_adapter("transfer", runtime_adapter.clone());
    // Attach this fresh replica before deliberately contaminating the fixture.
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        runtime_adapter.as_ref(),
        &sandbox,
        "test",
    )
    .await
    .unwrap();
    runtime_adapter
        .deliver_credential(
            &handle,
            layer3_adapter::CredentialFrame {
                operation_id: "fixture-contamination".into(),
                value: layer3_adapter::RuntimeCredential::new("opaque-native-fixture"),
                custody_generation: Some(1),
                github_route: None,
            },
        )
        .await
        .unwrap();
    let claimed = claim(&harness, &sandbox, "transfer", now).await;
    worker
        .adopt_initial_runtime(claimed, Some(&digest), now)
        .await
        .unwrap();
    assert_eq!(
        harness.stored(&sandbox.id).await.status,
        SandboxStatus::Failed
    );
    assert!(harness.actions.snapshot().is_empty());
}
