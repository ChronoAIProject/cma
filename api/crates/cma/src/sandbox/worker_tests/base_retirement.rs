use super::*;

#[tokio::test]
async fn delete_cleans_snapshots_before_retirement_and_preserves_history_until_grace() {
    let grace = Duration::from_secs(60);
    let harness = Harness::new(Vec::new(), 0, grace).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let deleter = caller("owner", [Capability::DeleteOwnSandbox]);
    let retiring = harness.service.delete(&deleter, &active.id).await.unwrap();
    assert_eq!(retiring.status, SandboxStatus::Retiring);

    for _ in 0..4 {
        harness.run(&created.id, now).await;
    }
    let retired = harness.stored(&created.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert!(retired.runtime.is_none());
    assert!(retired.credential.current.is_none());
    assert_eq!(retired.work.step, SandboxWorkStep::WaitForPurge);
    assert_eq!(retired.purge_after, Some(rfc3339_at(now + grace)));
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "stop-agents", "revoke:provider-key-1"]
    );
    assert!(!harness.worker.run_one(&created.id, now).await.unwrap());
    assert!(!harness
        .store
        .sandbox_events(&created.id, 0, 50)
        .await
        .unwrap()
        .is_empty());

    let purge_at = now + grace;
    harness.run(&created.id, purge_at).await;
    harness.run(&created.id, purge_at).await;
    assert!(harness
        .store
        .get_sandbox(&created.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "stop-agents", "revoke:provider-key-1"]
    );
    assert!(harness
        .store
        .list_sandboxes(&SubjectId::new("owner"), &PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn retirement_reads_current_policy_when_it_sets_the_purge_deadline() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    for _ in 0..3 {
        harness.run(&active.id, now).await;
    }
    assert_eq!(
        harness.stored(&active.id).await.work.step,
        SandboxWorkStep::MarkRetired
    );

    let source = Arc::new(MutablePolicy(Mutex::new(harness.policy.clone())));
    let policy_source: Arc<dyn SandboxPolicySource> = source.clone();
    let credentials: Arc<dyn SandboxCredentialController> = harness.credentials.clone();
    let worker = harness.worker_with_policy_source(
        "live-policy-worker",
        credentials,
        Duration::from_secs(60),
        policy_source,
    );
    source
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retention_grace = Duration::from_secs(120);
    assert!(worker.run_one(&active.id, now).await.unwrap());

    let retired = harness.stored(&active.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert_eq!(
        retired.purge_after,
        Some(rfc3339_at(now + Duration::from_secs(120)))
    );
}

#[tokio::test]
async fn failed_retirement_revocation_is_durable_debt_and_retried() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    harness.credentials.fail_revokes(1);
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();

    for _ in 0..2 {
        harness.run(&created.id, now).await;
    }
    let retiring_with_debt = harness.stored(&created.id).await;
    assert_eq!(retiring_with_debt.status, SandboxStatus::Retiring);
    assert_eq!(
        retiring_with_debt.work.step,
        SandboxWorkStep::RevokeRetirementCredentials
    );
    assert_eq!(retiring_with_debt.credential.revoke_debt.len(), 1);
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "stop-agents", "revoke:provider-key-1"]
    );
    assert!(retiring_with_debt.runtime.is_some());
    assert!(retiring_with_debt.purge_after.is_none());
    assert!(!harness.worker.run_one(&created.id, now).await.unwrap());
    let retry = stamp_time(&retiring_with_debt.work.ready_at);
    for _ in 0..3 {
        harness.run(&created.id, retry).await;
    }
    let settled = harness.stored(&created.id).await;
    assert!(settled.credential.revoke_debt.is_empty());
    assert_eq!(settled.work.step, SandboxWorkStep::WaitForPurge);
    assert_eq!(
        harness.actions.snapshot(),
        [
            "mint:1",
            "stop-agents",
            "revoke:provider-key-1",
            "revoke:provider-key-1"
        ]
    );
}

#[tokio::test]
async fn retirement_keeps_revoke_debt_until_custody_is_deleted() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let authority = active.credential.current.clone().unwrap();
    harness.credentials.restore(CredentialMaterial {
        source: Default::default(),
        github_route: None,
        sandbox_id: active.id.clone(),
        owner: active.owner.clone(),
        authority: authority.clone(),
        value: SecretValue::new("restored-custody".into()),
    });
    harness.credentials.fail_forgets(1);
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();

    for _ in 0..2 {
        harness.run(&active.id, now).await;
    }
    let retired = harness.stored(&active.id).await;
    assert_eq!(retired.status, SandboxStatus::Retiring);
    assert!(retired.runtime.is_some());
    assert!(retired.purge_after.is_none());
    assert_eq!(retired.credential.revoke_debt.len(), 1);
    assert!(harness
        .credentials
        .material(&active.id, authority.generation)
        .await
        .unwrap()
        .is_some());

    let retry = stamp_time(&retired.work.ready_at);
    for _ in 0..3 {
        harness.run(&active.id, retry).await;
    }
    let settled = harness.stored(&active.id).await;
    assert!(settled.credential.revoke_debt.is_empty());
    assert_eq!(settled.work.step, SandboxWorkStep::WaitForPurge);
    assert!(harness
        .credentials
        .material(&active.id, authority.generation)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn custody_that_appears_after_the_grace_check_blocks_purge_until_revoked() {
    let grace = Duration::from_secs(60);
    let harness = Harness::new(Vec::new(), 0, grace).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, now).await;
    }

    let purge_at = now + grace;
    harness.run(&active.id, purge_at).await;
    assert_eq!(
        harness.stored(&active.id).await.work.step,
        SandboxWorkStep::Purge
    );
    harness.credentials.restore(CredentialMaterial {
        source: Default::default(),
        github_route: None,
        sandbox_id: active.id.clone(),
        owner: active.owner.clone(),
        authority: layer2_core::sandbox::SandboxCredentialAuthority {
            generation: 2,
            provider_key_id: "late-provider-key".into(),
            provider_state_version: 2,
            issued_at: rfc3339_at(purge_at),
            expires_at: rfc3339_at(purge_at + Duration::from_secs(3_600)),
            grant_fingerprint: "b".repeat(64),
        },
        value: SecretValue::new("late-custody".into()),
    });

    harness.run(&active.id, purge_at).await;
    let deferred = harness.stored(&active.id).await;
    assert_eq!(deferred.work.step, SandboxWorkStep::RetryRetiredRevocations);
    assert_eq!(deferred.credential.revoke_debt.len(), 1);
    assert!(!harness
        .actions
        .snapshot()
        .iter()
        .any(|action| action.as_str() == "purge-checkpoints"));

    harness.run(&active.id, purge_at).await;
    harness.run(&active.id, purge_at).await;
    harness.run(&active.id, purge_at).await;
    assert!(harness
        .store
        .get_sandbox(&active.id)
        .await
        .unwrap()
        .is_none());
    assert!(harness
        .actions
        .snapshot()
        .contains(&"revoke:late-provider-key".into()));
}
