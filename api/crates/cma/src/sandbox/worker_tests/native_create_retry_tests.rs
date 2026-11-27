use super::*;
use layer3_adapter::agent_sandbox::NativeClaimGuardReason;

#[path = "native_create_retry_provenance_tests.rs"]
mod provenance;

fn cold_worker(h: &Harness, pools: Arc<Pools>, enabled: bool) -> Arc<SandboxWorker> {
    let adapter = Arc::new(
        CodexAdapter::with_backend(
            Backend {
                pools: pools.clone(),
                backend: h.backend.clone(),
            },
            CodexSandboxSpec::default(),
        )
        .with_home_volumes(pools),
    );
    let policy = layer3_adapter::HomeVolumePolicy {
        mode: layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "storage".into(),
        capacity_gib: 64,
        template_name: String::new(),
        template_generation: String::new(),
    };
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: h.store.clone(),
            adapter,
            credentials: h.credentials.clone(),
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
        SandboxWorkerConfig::new("native-test", "test", runtime_image(), h.policy.clone())
            .with_home_policy(Some(policy))
            .with_native_create_retry(enabled),
    )
}

fn pending_deadline() -> AdapterError {
    AdapterError::NativeClaimNotReady(NativeClaimGuardReason::Pending)
}

#[tokio::test(start_paused = true)]
async fn native_create_retry_cannot_overwrite_retirement_or_successor_authority_during_wait() {
    for retiring in [false, true] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        pools.pending_claim.store(true, Ordering::SeqCst);
        let worker = cold_worker(&h, pools.clone(), true);
        let id = created.id.clone();
        let now = start_time(&created);
        let job = tokio::spawn(async move { worker.run_one(&id, now).await });
        pools.observed.notified().await;
        if retiring {
            h.service
                .delete(
                    &caller("owner", [Capability::DeleteOwnSandbox]),
                    &created.id,
                )
                .await
                .unwrap();
        } else {
            let current = h.stored(&created.id).await;
            h.store
                .claim_sandbox_work(
                    &created.id,
                    "successor",
                    current.work.lease.unwrap().expires_at,
                    rfc3339_at(now + Duration::from_secs(7200)),
                )
                .await
                .unwrap()
                .unwrap();
        }
        let changed = h.stored(&created.id).await;
        assert_eq!(job.await.unwrap(), Err(SandboxStoreError::Conflict));
        assert_eq!(h.stored(&created.id).await, changed);
        assert_eq!(call_count(&pools, "claim"), 1);
        assert_eq!(call_count(&pools, "adopt"), 0);
        assert_eq!(call_count(&pools, "reclaim"), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn native_create_retry_observes_fourteen_second_cold_assignment_on_original_claim() {
    for enabled in [false, true] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        pools.pending_claim.store(true, Ordering::SeqCst);
        let now = start_time(&created);
        let start = Instant::now();
        let ready = async {
            tokio::time::sleep(Duration::from_secs(14)).await;
            pools.pending_claim.store(false, Ordering::SeqCst);
        };
        let drive = async {
            let mut original = None;
            let mut pending_home = None;
            for _ in 0..6 {
                // A fresh worker each attempt proves the budget/authority are durable.
                let worker = cold_worker(&h, pools.clone(), enabled);
                assert!(worker
                    .run_one(&created.id, now + start.elapsed())
                    .await
                    .unwrap());
                let stored = h.stored(&created.id).await;
                let home = stored.lifecycle.home.as_deref().unwrap();
                let native = home.native_pool.as_ref().unwrap();
                let claim = native.claim.as_ref().unwrap();
                assert!(native.cleanup.is_none() && !native.cleaned && !home.released);
                if let Some(original) = &original {
                    assert_eq!(&claim.uid, original);
                } else {
                    original = Some(claim.uid.clone());
                }
                if stored.runtime.is_some() {
                    assert_eq!(claim.sandbox_name.as_deref(), Some("warm-member"));
                    let receipt = home.receipt.as_ref().unwrap();
                    assert_eq!(receipt.storage_generation, "pvc-uid");
                    let authority = receipt.native_authority.as_ref().unwrap();
                    assert_eq!(authority.claim, *claim);
                    assert_eq!(authority.sandbox_uid, "member-uid");
                    assert_eq!(authority.pvc_uid, "pvc-uid");
                    return start.elapsed();
                }
                if let Some(previous) = &pending_home {
                    assert_eq!(home, previous);
                }
                pending_home = Some(home.clone());
                let due = Duration::from_secs(
                    seconds_from_rfc3339(&stored.work.ready_at).unwrap()
                        - now
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                );
                if enabled {
                    assert_eq!(now + due, now + start.elapsed() + Duration::from_secs(1));
                    assert!(!worker
                        .run_one(&created.id, now + start.elapsed())
                        .await
                        .unwrap());
                }
                tokio::time::sleep(due.saturating_sub(start.elapsed())).await;
            }
            panic!("cold preparation did not finish")
        };
        let (elapsed, ()) = tokio::join!(drive, ready);
        if enabled {
            // The readiness transition and a poll can wake on the same tick;
            // the next 250 ms poll is still the intended observation bound.
            assert!((Duration::from_secs(14)..=Duration::from_millis(14_250)).contains(&elapsed));
        } else {
            assert_eq!(elapsed, Duration::from_secs(30));
        }
        assert_eq!(call_count(&pools, "claim"), 1);
        assert_eq!(call_count(&pools, "select"), 1);
        assert_eq!(call_count(&pools, "adopt"), 1);
        assert_eq!(call_count(&pools, "remove"), 0);
        assert_eq!(call_count(&pools, "reclaim"), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn native_create_retry_budget_survives_worker_replacement_without_resetting_attempts() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let pools = pools(&h, &[]);
    pools.pending_claim.store(true, Ordering::SeqCst);
    let mut now = start_time(&created) + Duration::from_millis(500);
    let mut original = None;
    for attempt in 0..7 {
        assert!(cold_worker(&h, pools.clone(), true)
            .run_one(&created.id, now)
            .await
            .unwrap());
        let stored = h.stored(&created.id).await;
        assert_eq!(stored.work.attempts, attempt + 1);
        assert_eq!(stored.status, SandboxStatus::Provisioning);
        if let Some(home) = &original {
            assert_eq!(&stored.lifecycle.home, home);
        } else {
            original = Some(stored.lifecycle.home.clone());
        }
        // The first five retries start at the rounded completion of the
        // readiness observation. Later retries use the ordinary backoff.
        let observed_at = stamp_time(&stored.updated_at);
        let delay = if attempt < 5 {
            assert!(observed_at >= now + Duration::from_secs(5));
            Duration::from_secs(1)
        } else {
            assert!(observed_at >= now);
            Duration::from_secs(30)
        };
        assert_eq!(stored.work.ready_at, rfc3339_at(observed_at + delay));
        now = stamp_time(&stored.work.ready_at);
    }
    assert_eq!(call_count(&pools, "claim"), 1);
    assert_eq!(call_count(&pools, "adopt"), 0);
}

#[tokio::test(start_paused = true)]
async fn native_create_retry_excludes_crashes_credentials_retirement_and_recovery() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let pools = pools(&h, &[]);
    pools.pending_claim.store(true, Ordering::SeqCst);
    let worker = cold_worker(&h, pools.clone(), true);
    worker
        .run_one(&created.id, start_time(&created))
        .await
        .unwrap();
    let original = h.stored(&created.id).await;
    assert_eq!(
        worker.native_retry_delay_for_test(&original, pending_deadline()),
        Duration::from_secs(1)
    );
    for error in [
        AdapterError::NativeClaimNotReady(NativeClaimGuardReason::Unschedulable),
        AdapterError::NativeClaimNotReady(NativeClaimGuardReason::CrashLoop),
        AdapterError::NativeClaimNotReady(NativeClaimGuardReason::Restarting),
        AdapterError::Unavailable("transport failure".into()),
        AdapterError::Rejected("claim UID changed".into()),
    ] {
        assert_eq!(
            worker.native_retry_delay_for_test(&original, error),
            Duration::from_secs(30)
        );
    }
    let authority = layer2_core::sandbox::SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: "fixture".into(),
        provider_state_version: 1,
        issued_at: created.created_at.clone(),
        expires_at: String::new(),
        grant_fingerprint: "a".repeat(64),
    };
    for case in [
        "no_home",
        "no_claim",
        "cleaning",
        "cleaned",
        "released",
        "intent",
        "pending",
        "current",
        "delivered",
        "retained",
        "retiring",
        "active",
        "installing",
        "lifecycle",
        "budget",
    ] {
        let mut candidate = original.clone();
        match case {
            "no_home" => candidate.lifecycle.home = None,
            "no_claim" => {
                candidate
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .native_pool
                    .as_mut()
                    .unwrap()
                    .claim = None
            }
            "cleaning" => {
                let native = candidate
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .native_pool
                    .as_mut()
                    .unwrap();
                native.cleanup = Some(NativePoolCleanup {
                    request: native.request.clone(),
                    claim: native.claim.clone(),
                    members: vec![],
                });
            }
            "cleaned" => {
                candidate
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .native_pool
                    .as_mut()
                    .unwrap()
                    .cleaned = true
            }
            "released" => candidate.lifecycle.home.as_mut().unwrap().released = true,
            "intent" => candidate.credential.key_generation = 1,
            "pending" => candidate.credential.pending = Some(authority.clone()),
            "current" => candidate.credential.current = Some(authority.clone()),
            "delivered" => candidate.credential.delivered = true,
            "retained" => candidate.lifecycle.runtime_generation_high_water = 2,
            "retiring" => candidate.status = SandboxStatus::Retiring,
            "active" => candidate.status = SandboxStatus::Active,
            "installing" => candidate.status = SandboxStatus::Installing,
            "lifecycle" => candidate.work.step = SandboxWorkStep::RunLifecycleOperation,
            "budget" => candidate.work.attempts = 5,
            _ => unreachable!(),
        }
        assert_eq!(
            worker.native_retry_delay_for_test(&candidate, pending_deadline()),
            Duration::from_secs(30),
            "{case}"
        );
    }
    assert_eq!(h.stored(&created.id).await, original);
    assert_eq!(call_count(&pools, "claim"), 1);
    assert_eq!(call_count(&pools, "adopt"), 0);
}
