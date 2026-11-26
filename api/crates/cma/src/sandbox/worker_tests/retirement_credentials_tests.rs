use super::*;

#[tokio::test]
async fn retirement_settles_all_sources_and_retries_only_remaining_exact_keys() {
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let now = start_time(&created);
    let mut active = h.drive_active(&created.id, now).await;
    let revision = active.revision;
    let current = h
        .credentials
        .material(&active.id, 1)
        .await
        .unwrap()
        .unwrap();
    let mut pending = current.clone();
    pending.authority.generation = 2;
    pending.authority.provider_key_id = "provider-key-2".into();
    active.credential.pending = Some(pending.authority.clone());
    active.credential.key_generation = 2;
    h.credentials.restore(pending);
    let mut custody_only = current;
    custody_only.authority.generation = 3;
    custody_only.authority.provider_key_id = "provider-key-3".into();
    h.credentials.restore(custody_only);
    active
        .credential
        .queue_revoke("provider-key-4".into(), rfc3339_at(now));
    h.credentials
        .live_keys
        .lock()
        .unwrap()
        .insert("provider-key-4".into());
    active.revision += 1;
    h.store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    h.credentials
        .revoke_failure_ids
        .lock()
        .unwrap()
        .extend(["provider-key-2".into(), "provider-key-4".into()]);
    h.service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    h.run(&active.id, now).await;
    h.run(&active.id, now).await;
    let failed = h.stored(&active.id).await;
    assert_eq!(failed.status, SandboxStatus::Retiring);
    assert_eq!(
        failed.work.step,
        SandboxWorkStep::RevokeRetirementCredentials
    );
    assert!(failed.credential.current.is_none());
    assert!(failed.credential.pending.is_none());
    assert_eq!(
        failed
            .credential
            .revoke_debt
            .iter()
            .map(|d| d.provider_key_id.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["provider-key-2", "provider-key-4"])
    );
    assert!(failed.runtime.is_some());
    assert!(h
        .credentials
        .material(&active.id, 3)
        .await
        .unwrap()
        .is_none());
    h.credentials
        .revoke_failure_ids
        .lock()
        .unwrap()
        .remove("provider-key-2");
    let retry = stamp_time(&failed.work.ready_at);
    // A newly constructed worker resumes from durable parent-only debt.
    let successor = h.worker_with("successor", h.credentials.clone(), Duration::from_secs(60));
    assert!(successor.run_one(&active.id, retry).await.unwrap());
    let failed = h.stored(&active.id).await;
    assert_eq!(failed.credential.revoke_debt.len(), 1);
    assert_eq!(
        failed.credential.revoke_debt[0].provider_key_id,
        "provider-key-4"
    );
    h.credentials.revoke_failure_ids.lock().unwrap().clear();
    let retry = stamp_time(&failed.work.ready_at);
    for _ in 0..3 {
        assert!(successor.run_one(&active.id, retry).await.unwrap());
    }
    assert_eq!(h.stored(&active.id).await.status, SandboxStatus::Retired);
    assert!(h.credentials.live_keys.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unavailable_or_untrusted_retirement_census_retains_authority_and_paces_retry() {
    for failure in ["census", "settlement", "owner", "source", "sandbox"] {
        let h = Harness::new(Vec::new(), 0, Duration::ZERO).await;
        let created = h.create().await;
        let now = start_time(&created);
        let active = h.drive_active(&created.id, now).await;
        match failure {
            "census" => {
                *h.credentials.census_failure.lock().unwrap() =
                    Some(CredentialError::Unavailable("census outage".into()))
            }
            "settlement" => {
                *h.credentials.settlement_failure.lock().unwrap() = Some(CredentialError::Uncertain)
            }
            _ => {
                let mut rows = h.credentials.materials.lock().unwrap();
                let material = rows.values_mut().next().unwrap();
                match failure {
                    "owner" => material.owner = SubjectId::new("different-owner"),
                    "sandbox" => material.sandbox_id = SandboxId("different-sandbox".into()),
                    _ => {
                        material.source = layer2_core::sandbox::SandboxCredentialSource::Provided {
                            key_sha256: "a".repeat(64),
                        }
                    }
                }
            }
        }
        h.service
            .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
            .await
            .unwrap();
        h.run(&active.id, now).await;
        h.run(&active.id, now).await;
        let blocked = h.stored(&active.id).await;
        assert_eq!(blocked.status, SandboxStatus::Retiring, "{failure}");
        assert_eq!(
            blocked.work.step,
            SandboxWorkStep::RevokeRetirementCredentials
        );
        assert_eq!(blocked.credential.current, active.credential.current);
        assert_eq!(blocked.runtime, active.runtime);
        assert!(stamp_time(&blocked.work.ready_at) > now);
        assert!(!h.worker.run_one(&active.id, now).await.unwrap());
        assert!(!h
            .actions
            .snapshot()
            .iter()
            .any(|a| a.starts_with("revoke:")));
    }
}

#[tokio::test]
async fn permanent_revocation_outage_finishes_retry_budget_and_retains_custody() {
    use layer2_core::sandbox::admin::{SandboxRetirement, StoredSandboxRetirement};
    use layer2_core::sandbox::work_retry::WORK_RETRY_BUDGET;

    let h = Harness::new(Vec::new(), 0, Duration::ZERO).await;
    let created = h.create().await;
    let mut now = start_time(&created);
    let active = h.drive_active(&created.id, now).await;
    h.credentials.fail_revokes(100);
    h.service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    h.run(&active.id, now).await;
    for _ in 0..WORK_RETRY_BUDGET {
        h.run(&active.id, now).await;
        let blocked = h.stored(&active.id).await;
        assert_eq!(blocked.status, SandboxStatus::Retiring);
        assert_eq!(
            blocked.work.step,
            SandboxWorkStep::RevokeRetirementCredentials
        );
        assert_eq!(blocked.credential.revoke_debt.len(), 1);
        assert_eq!(blocked.runtime, active.runtime);
        assert!(blocked.purge_after.is_none());
        assert!(!h.worker.run_one(&active.id, now).await.unwrap());
        now = stamp_time(&blocked.work.ready_at);
    }
    h.run(&active.id, now).await;
    let terminal = h.stored(&active.id).await;
    assert!(terminal.retirement_retry_exhausted());
    assert_eq!(terminal.work.attempts, WORK_RETRY_BUDGET + 1);
    assert_eq!(terminal.credential.revoke_debt.len(), 1);
    assert_eq!(terminal.runtime, active.runtime);
    assert!(terminal.purge_after.is_none());
    let restarted = h.worker_with("retirement-successor", h.credentials.clone(), Duration::ZERO);
    assert!(!restarted
        .run_one(&active.id, now + Duration::from_secs(3600))
        .await
        .unwrap());
    h.credentials.fail_revokes(0);
    let retry_at = now + Duration::from_secs(3600);
    let resumed = StoredSandboxRetirement::new(h.store.clone())
        .retire(&active.id, rfc3339_at(retry_at), Some(terminal.revision))
        .await
        .unwrap();
    assert!(resumed.changed);
    assert!(restarted.run_one(&active.id, retry_at).await.unwrap());
    let settled = h.stored(&active.id).await;
    assert_eq!(settled.work.step, SandboxWorkStep::TeardownRuntime);
    assert!(settled.credential.revoke_debt.is_empty());
    assert_eq!(settled.runtime, active.runtime);
    assert!(!settled.retirement_released_owner_slot());
}

#[tokio::test]
async fn missing_provider_authority_cannot_authorize_cleanup_from_empty_parent_metadata() {
    let h = Harness::new(Vec::new(), 0, Duration::ZERO).await;
    let created = h.create().await;
    let now = start_time(&created);
    let mut row = h.drive_active(&created.id, now).await;
    let revision = row.revision;
    row.begin_retirement(rfc3339_at(now)).unwrap();
    row.work.step = SandboxWorkStep::TeardownRuntime;
    row.credential.current = None;
    row.credential.pending = None;
    row.credential.delivered = false;
    h.store.save_sandbox(revision, row.clone()).await.unwrap();
    let worker = h.worker_with(
        "without-provider-authority",
        Arc::new(crate::sandbox::UnavailableSandboxCredentials),
        Duration::from_secs(60),
    );
    assert!(worker.run_one(&row.id, now).await.unwrap());
    let blocked = h.stored(&row.id).await;
    assert_eq!(blocked.status, SandboxStatus::Retiring);
    assert_eq!(blocked.work.step, SandboxWorkStep::TeardownRuntime);
    assert_eq!(blocked.runtime, row.runtime);
    assert!(blocked.purge_after.is_none());
    assert!(stamp_time(&blocked.work.ready_at) > now);
    assert!(!worker.run_one(&row.id, now).await.unwrap());
}
