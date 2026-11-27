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
    use layer2_core::sandbox::work_retry::WORK_RETRY_BUDGET;
    use layer2_core::sandbox::{SandboxRetirement, StoredSandboxRetirement};

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
    let restarted = h.worker_with(
        "retirement-successor",
        h.credentials.clone(),
        Duration::from_secs(60),
    );
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
async fn exhausted_retirement_records_unresolved_issuance_and_retains_the_slot() {
    use layer2_core::sandbox::work_retry::WORK_RETRY_BUDGET;

    let h = Harness::new(Vec::new(), 0, Duration::ZERO).await;
    let created = h.create().await;
    let mut now = start_time(&created);
    let active = h.drive_active(&created.id, now).await;
    *h.credentials.settlement_failure.lock().unwrap() = Some(CredentialError::Uncertain);
    h.service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    h.run(&active.id, now).await;
    for _ in 0..=WORK_RETRY_BUDGET {
        h.run(&active.id, now).await;
        let stored = h.stored(&active.id).await;
        if !stored.retirement_retry_exhausted() {
            now = stamp_time(&stored.work.ready_at);
        }
    }
    let stored = h.stored(&active.id).await;
    assert!(stored.retirement_retry_exhausted());
    assert!(!stored.retirement_released_owner_slot());
    let events = h.store.sandbox_events(&active.id, 0, 100).await.unwrap();
    assert!(matches!(
        &events.last().unwrap().kind,
        SandboxEventKind::WorkStepFailed {
            step: SandboxWorkStep::RevokeRetirementCredentials,
            failure,
        } if failure.class == "credential_issuance_unresolved"
    ));
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

#[tokio::test]
async fn verified_reauthorization_requeues_parked_retirement_once_across_workers() {
    use layer2_core::sandbox::RetirementAuthorityVersion;

    for lose_reply in [false, true] {
        let h = Harness::new(Vec::new(), 0, Duration::from_secs(30)).await;
        let parked = park_for_owner_reauthorization(&h, true).await;
        let id = &parked.id;
        let recovery = parked.work.retirement_credential_recovery.unwrap();
        assert!(recovery.admitted.is_none());
        assert_eq!(recovery.expected_scope, [3; 32]);
        assert_eq!(parked.credential.revoke_debt.len(), 1);
        let old = RetirementAuthorityVersion {
            grant: [1; 16],
            scope: [3; 32],
            granted_at_ms: recovery.newer_than_ms,
        };
        *h.credentials.retirement_authority.lock().unwrap() = Some(old);
        h.run(id, stamp_time(&parked.work.ready_at)).await;
        let waiting = h.stored(id).await;
        assert_eq!(waiting.work.retirement_credential_recovery, Some(recovery));
        assert_eq!(
            waiting.credential.revoke_debt,
            parked.credential.revoke_debt
        );
        let different = RetirementAuthorityVersion {
            grant: [2; 16],
            scope: [4; 32],
            granted_at_ms: recovery.newer_than_ms + 1,
        };
        *h.credentials.retirement_authority.lock().unwrap() = Some(different);
        h.run(id, stamp_time(&waiting.work.ready_at)).await;
        let still_waiting = h.stored(id).await;
        assert_eq!(
            still_waiting.work.retirement_credential_recovery,
            Some(recovery)
        );
        assert_eq!(
            still_waiting.credential.revoke_debt,
            parked.credential.revoke_debt
        );
        assert!(!h
            .actions
            .snapshot()
            .iter()
            .any(|action| action.starts_with("revoke:")));
        let fresh = RetirementAuthorityVersion {
            grant: [3; 16],
            scope: [3; 32],
            granted_at_ms: different.granted_at_ms + 1,
        };
        *h.credentials.retirement_authority.lock().unwrap() = Some(fresh);
        let due = stamp_time(&still_waiting.work.ready_at);
        let peer = h.worker_with("peer", h.credentials.clone(), Duration::from_secs(30));
        // The persisted acknowledgement can be observed before either worker dispatches.
        h.run(id, due).await;
        let admitted = h.stored(id).await;
        assert_eq!(
            admitted
                .work
                .retirement_credential_recovery
                .unwrap()
                .admitted,
            Some(fresh)
        );
        assert_eq!(
            admitted.credential.revoke_debt,
            parked.credential.revoke_debt
        );
        assert_eq!(
            admitted.work.step,
            SandboxWorkStep::RevokeRetirementCredentials
        );
        h.credentials
            .lose_retirement_reply
            .store(lose_reply, Ordering::SeqCst);
        let successor = h.worker_with("successor", h.credentials.clone(), Duration::from_secs(30));
        let prior_attempts = h
            .actions
            .snapshot()
            .into_iter()
            .filter(|action| action.starts_with("revoke:"))
            .count();
        let (first, second) = tokio::join!(peer.run_one(id, due), successor.run_one(id, due));
        assert!(first.unwrap() || second.unwrap());
        let current_attempts = h
            .actions
            .snapshot()
            .into_iter()
            .filter(|action| action.starts_with("revoke:"))
            .count();
        assert_eq!(current_attempts, prior_attempts + 1);
        let after_dispatch = h.stored(id).await;
        if lose_reply {
            assert_eq!(
                after_dispatch.credential.revoke_debt,
                parked.credential.revoke_debt
            );
            assert!(!after_dispatch.retirement_released_owner_slot());
            let retry = stamp_time(&after_dispatch.work.ready_at);
            assert!(successor.run_one(id, retry).await.unwrap());
            let ambiguous = h.stored(id).await;
            assert_eq!(
                ambiguous.credential.revoke_debt,
                parked.credential.revoke_debt
            );
            assert_eq!(ambiguous.status, SandboxStatus::Retiring);
            assert!(!ambiguous.retirement_released_owner_slot());
        } else {
            assert_eq!(after_dispatch.credential.revoke_debt.len(), 0);
            for _ in 0..3 {
                let row = h.stored(id).await;
                if row.status == SandboxStatus::Retired {
                    break;
                }
                assert!(successor
                    .run_one(id, stamp_time(&row.work.ready_at))
                    .await
                    .unwrap());
            }
            assert_eq!(h.stored(id).await.status, SandboxStatus::Retired);
        }
    }
}

#[tokio::test]
async fn operator_can_resume_waiting_retirement_without_clearing_debt() {
    use layer2_core::sandbox::{SandboxRetirement, StoredSandboxRetirement};

    let h = Harness::new(Vec::new(), 0, Duration::from_secs(30)).await;
    let parked = park_for_owner_reauthorization(&h, true).await;
    let retirement = StoredSandboxRetirement::new(h.store.clone());
    let at = parked.updated_at.clone();
    assert!(retirement
        .retire(&parked.id, at.clone(), Some(parked.revision - 1))
        .await
        .is_err());
    assert!(
        !retirement
            .retire(&parked.id, at.clone(), None)
            .await
            .unwrap()
            .changed
    );
    let resumed = retirement
        .retire(&parked.id, at, Some(parked.revision))
        .await
        .unwrap();
    assert!(resumed.changed);
    assert!(resumed
        .sandbox
        .work
        .retirement_credential_recovery
        .is_none());
    assert_eq!(
        resumed.sandbox.credential.revoke_debt,
        parked.credential.revoke_debt
    );
}

#[tokio::test]
async fn missing_owner_credential_without_issuance_scope_stays_parked_for_operations() {
    use layer2_core::sandbox::RetirementAuthorityVersion;

    let h = Harness::new(Vec::new(), 0, Duration::from_secs(30)).await;
    let parked = park_for_owner_reauthorization(&h, false).await;
    assert!(parked.retirement_retry_exhausted());
    assert!(parked.work.retirement_credential_recovery.is_none());
    assert_eq!(parked.credential.revoke_debt.len(), 1);
    *h.credentials.retirement_authority.lock().unwrap() = Some(RetirementAuthorityVersion {
        grant: [2; 16],
        scope: [3; 32],
        granted_at_ms: u64::MAX,
    });
    assert!(!h
        .worker
        .run_one(
            &parked.id,
            stamp_time(&parked.updated_at) + Duration::from_secs(3600)
        )
        .await
        .unwrap());
    assert!(!h
        .actions
        .snapshot()
        .iter()
        .any(|action| action.starts_with("revoke:")));
}

async fn park_for_owner_reauthorization(h: &Harness, issued_scope_known: bool) -> Sandbox {
    use layer2_core::sandbox::RetirementAuthorityVersion;
    let created = h.create().await;
    let now = start_time(&created);
    let active = h.drive_active(&created.id, now).await;
    *h.credentials.retirement_issued_authority.lock().unwrap() =
        issued_scope_known.then_some(RetirementAuthorityVersion {
            grant: [1; 16],
            scope: [3; 32],
            granted_at_ms: 1,
        });
    h.credentials
        .require_retirement_authority
        .store(true, Ordering::SeqCst);
    h.service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    h.run(&active.id, now).await;
    for _ in 0..=layer2_core::sandbox::work_retry::WORK_RETRY_EXTERNAL_BUDGET {
        let row = h.stored(&active.id).await;
        h.run(&active.id, stamp_time(&row.work.ready_at)).await;
    }
    h.stored(&active.id).await
}
