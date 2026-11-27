use super::*;
use crate::agent_ownership::{test_support::MemoryOwnershipStore, Member};

async fn coordinator() -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register_with_policy(
        Arc::new(MemoryOwnershipStore::at_fixed_time(1)),
        Member {
            instance_id: "owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://owner.invalid".into(),
            expires_at_ms: 30_000,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 1,
        },
        false,
        MaintenancePolicy::ContainFailures,
    )
    .await
    .unwrap()
}

fn token(agent: &AgentId, epoch: u64) -> OwnershipToken {
    OwnershipToken {
        agent_id: agent.clone(),
        instance_id: "owner".into(),
        protocol: PROTOCOL_VERSION,
        epoch,
    }
}

fn install_pending(manager: &OwnershipCoordinator, id: &AgentId) -> Arc<LeaseWrite> {
    let task = LeaseWrite::renewal_with(async { Err(OwnershipError::Stale) });
    manager
        .owned
        .lock()
        .unwrap()
        .entry(id.clone())
        .or_default()
        .acquisition = Some(task.clone());
    task
}

#[tokio::test(start_paused = true)]
async fn delayed_acquisition_cannot_publish_with_lost_membership_or_wrong_identity() {
    let manager = coordinator().await;
    let id = AgentId("agt_test".into());
    let started = Instant::now();
    let valid = OwnershipLease {
        token: token(&id, 1),
        expires_at_ms: 15_001,
    };
    for invalid in [
        OwnershipToken {
            agent_id: AgentId("agt_other".into()),
            ..valid.token.clone()
        },
        OwnershipToken {
            instance_id: "other".into(),
            ..valid.token.clone()
        },
        OwnershipToken {
            protocol: PROTOCOL_VERSION + 1,
            ..valid.token.clone()
        },
        token(&id, 0),
    ] {
        let lease = OwnershipLease {
            token: invalid,
            ..valid.clone()
        };
        assert!(matches!(
            manager.acquired_deadline(&id, &lease, started, 1),
            Err(OwnershipError::Invalid(_))
        ));
    }
    for (expires_at_ms, now_ms) in [(1, 1), (i64::MAX, -1)] {
        let lease = OwnershipLease {
            expires_at_ms,
            ..valid.clone()
        };
        assert!(matches!(
            manager.acquired_deadline(&id, &lease, started, now_ms),
            Err(OwnershipError::Invalid(_))
        ));
    }

    let pending = install_pending(&manager, &id);
    *manager.member_deadline.lock().unwrap() = started;
    let result = manager.apply_acquisition(
        &id,
        &pending,
        WriteOutcome::Acquired {
            started,
            now_ms: 1,
            lease: Some(valid.clone()),
        },
        None,
        true,
    );
    assert!(result.unwrap().is_none());
    assert!(manager.local(&id).await.is_none());
    let owned = manager.owned.lock().unwrap();
    let retained = owned[&id].authority.as_ref().unwrap();
    assert_eq!(&retained.token, &valid.token);
    assert!(retained.retirement_pending());
    assert!(owned[&id].acquisition.is_none());
}

#[tokio::test(start_paused = true)]
async fn stale_acquisition_results_do_not_replace_pending_or_newer_epoch_custody() {
    let manager = coordinator().await;
    let id = AgentId("agt_test".into());
    let obsolete = install_pending(&manager, &id);
    let pending = install_pending(&manager, &id);
    assert!(matches!(
        manager.apply_acquisition(
            &id,
            &obsolete,
            WriteOutcome::ReconciledAcquisition(None),
            None,
            true
        ),
        Err(OwnershipError::Stale)
    ));
    assert!(Arc::ptr_eq(
        manager.owned.lock().unwrap()[&id]
            .acquisition
            .as_ref()
            .unwrap(),
        &pending
    ));

    let authority = Authority::unpublished(token(&id, 3), manager.store.clone(), manager.policy);
    manager
        .owned
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .authority = Some(authority.clone());
    assert!(manager
        .apply_acquisition(
            &id,
            &pending,
            WriteOutcome::ReconciledAcquisition(Some(token(&id, 2))),
            None,
            true,
        )
        .unwrap()
        .is_none());
    assert!(Arc::ptr_eq(
        manager.owned.lock().unwrap()[&id]
            .authority
            .as_ref()
            .unwrap(),
        &authority
    ));

    let pending = install_pending(&manager, &id);
    let mut cleanup = CleanupState {
        settlement: Some(Settlement::Superseded { newer_epoch: 5 }),
        ..CleanupState::default()
    };
    assert!(manager
        .apply_acquisition(
            &id,
            &pending,
            WriteOutcome::ReconciledAcquisition(Some(token(&id, 4))),
            Some(&mut cleanup),
            false,
        )
        .unwrap()
        .is_none());
    assert!(!manager.owned.lock().unwrap().contains_key(&id));
    assert_eq!(
        cleanup.settlement,
        Some(Settlement::Superseded { newer_epoch: 5 })
    );
}

#[tokio::test(start_paused = true)]
async fn acquisition_defers_under_contention_and_does_not_spend_capacity_on_a_refusal() {
    let manager = coordinator().await;
    let id = AgentId("agt_test".into());
    let other = AgentId("agt_other".into());
    let lock = manager.acquisition.lock().await;
    assert!(manager.acquire(&id).await.unwrap().is_none());
    assert!(manager.owned.lock().unwrap().is_empty());
    drop(lock);

    manager.skipped_heartbeats.store(1, Ordering::Release);
    assert!(manager.acquire(&id).await.unwrap().is_none());
    assert!(manager.start_acquisition(&id, None).is_none());
    manager.skipped_heartbeats.store(0, Ordering::Release);

    let pending = install_pending(&manager, &id);
    assert!(manager.start_acquisition(&other, None).is_none());
    for error in [
        OwnershipError::Stale,
        OwnershipError::Unavailable("fixture".into()),
    ] {
        let result = manager.apply_acquisition(
            &id,
            &pending,
            WriteOutcome::NotIssued(error.clone()),
            None,
            true,
        );
        match &error {
            OwnershipError::Unavailable(_) => assert!(result.unwrap().is_none()),
            error => assert!(matches!(result, Err(found) if found == *error)),
        }
        assert!(manager.owned.lock().unwrap().is_empty());
        if error == OwnershipError::Stale {
            manager
                .owned
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_default()
                .acquisition = Some(pending.clone());
        }
    }
    assert!(manager.acquire(&other).await.unwrap().is_some());
}

#[tokio::test(start_paused = true)]
async fn an_expired_acquisition_is_retained_only_for_settlement() {
    let manager = coordinator().await;
    let id = AgentId("agt_expired".into());
    let pending = install_pending(&manager, &id);
    let started = Instant::now();
    tokio::time::advance(Duration::from_secs(1)).await;
    let lease = OwnershipLease {
        token: token(&id, 4),
        expires_at_ms: 500,
    };
    assert!(manager
        .apply_acquisition(
            &id,
            &pending,
            WriteOutcome::Acquired {
                started,
                now_ms: 1,
                lease: Some(lease.clone())
            },
            None,
            true,
        )
        .unwrap()
        .is_none());
    let retained = manager.owned().await.pop().unwrap();
    assert_eq!(retained.token, lease.token);
    assert!(!retained.is_current());
    assert!(retained.retirement_pending());

    let pending = install_pending(&manager, &id);
    let mut cleanup = CleanupState::default();
    assert!(manager
        .apply_acquisition(
            &id,
            &pending,
            WriteOutcome::ReconciledAcquisition(None),
            Some(&mut cleanup),
            true,
        )
        .unwrap()
        .is_none());
    assert_eq!(manager.owned().await.len(), 1);
    let pending = install_pending(&manager, &id);
    cleanup.settlement = Some(Settlement::Released);
    assert!(manager
        .apply_acquisition(
            &id,
            &pending,
            WriteOutcome::ReconciledAcquisition(None),
            Some(&mut cleanup),
            true,
        )
        .unwrap()
        .is_none());
    assert!(manager.owned().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn joining_an_abandoned_store_write_keeps_its_epoch_for_cleanup() {
    let manager = coordinator().await;
    let id = AgentId("agt_abandoned".into());
    let _abandoned = manager.start_acquisition(&id, None).unwrap();
    assert!(manager.acquire(&id).await.unwrap().is_none());
    let retained = manager.owned().await.pop().unwrap();
    let stored = manager.store.owner(&id).await.unwrap().unwrap();
    assert_eq!(retained.token, stored.token);
    assert!(retained.retirement_pending());
    assert!(manager.local(&id).await.is_none());
    assert!(!retained.is_current());
    assert!(manager
        .start_acquisition(&AgentId("agt_other".into()), Some(&retained))
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn membership_loss_stops_new_acquisition_without_dropping_existing_custody() {
    for disposition in ["expired", "draining", "failed"] {
        let manager = coordinator().await;
        let id = AgentId("agt_existing".into());
        let retained = manager.acquire(&id).await.unwrap().unwrap();
        match disposition {
            "expired" => *manager.member_deadline.lock().unwrap() = Instant::now(),
            "draining" => manager.draining.store(true, Ordering::Release),
            "failed" => {
                manager.failed.send_replace(true);
            }
            _ => unreachable!(),
        }
        assert!(manager.start_acquisition(&id, Some(&retained)).is_none());
        assert!(matches!(
            manager.acquire(&id).await,
            Err(OwnershipError::Stale)
        ));
        assert!(Arc::ptr_eq(&manager.owned().await[0], &retained));
        assert_eq!(
            manager.store.owner(&id).await.unwrap().unwrap().token,
            retained.token
        );
    }
}
