use super::*;
use layer2_core::sandbox::home_backup::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "worker_fixture.rs"]
mod fixture;
use fixture::Fixture;

#[tokio::test]
async fn orphan_discovery_respects_admission_and_failure_leaves_deletion_recovery_available() {
    for (enabled, outcome) in [
        (false, "success"),
        (true, "success"),
        (true, "inventory_error"),
    ] {
        let fixture = Fixture::new(outcome);
        let worker = ExpiryWorker::new(
            enabled,
            fixture.clone(),
            Some(fixture.clone()),
            Some(orphans::OrphanSources {
                inventory: fixture.clone(),
                catalog: fixture.clone(),
            }),
            "expiry-fixture",
        )
        .unwrap();
        let result = worker.reconcile_orphan_page().await;
        assert_eq!(result.is_err(), outcome == "inventory_error");
        assert_eq!(
            *fixture.calls.lock().unwrap(),
            if enabled { vec!["list"] } else { vec![] }
        );
        if outcome == "inventory_error" {
            assert!(worker.reconcile_one().await.unwrap());
            assert_eq!(
                fixture.backup.lock().unwrap().state,
                HomeBackupState::Deleted
            );
        }
    }
}

#[tokio::test]
async fn disabled_worker_performs_no_discovery_or_provider_calls() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(false, fixture.clone(), None, None, "expiry-fixture").unwrap();
    assert!(!worker.reconcile_one().await.unwrap());
    assert!(fixture.calls.lock().unwrap().is_empty());
}

#[test]
fn enabled_worker_requires_a_real_provider_boundary() {
    assert!(
        ExpiryWorker::new(true, Fixture::new("success"), None, None, "expiry-fixture").is_err()
    );
}

#[tokio::test]
async fn worker_persists_authority_then_deletes_observes_and_publishes_exact_receipt() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(
        true,
        fixture.clone(),
        Some(fixture.clone()),
        None,
        "expiry-fixture",
    )
    .unwrap();
    assert!(worker.reconcile_one().await.unwrap());
    assert_eq!(
        *fixture.calls.lock().unwrap(),
        vec!["claim", "delete", "observe", "save"]
    );
    assert_eq!(
        fixture.backup.lock().unwrap().state,
        HomeBackupState::Deleted
    );
}

#[tokio::test]
async fn provider_errors_or_present_copy_preserve_expiring_and_do_not_record_deletion() {
    for outcome in ["delete_error", "observe_error", "present"] {
        let fixture = Fixture::new(outcome);
        let worker = ExpiryWorker::new(
            true,
            fixture.clone(),
            Some(fixture.clone()),
            None,
            "expiry-fixture",
        )
        .unwrap();
        assert!(worker.reconcile_one().await.is_err());
        assert_eq!(
            fixture.backup.lock().unwrap().state,
            HomeBackupState::Expiring
        );
        assert!(fixture.backup.lock().unwrap().erasure.is_none());
        assert!(!fixture.calls.lock().unwrap().contains(&"save"));
    }
}

#[tokio::test]
async fn worker_refuses_an_external_step_when_the_remaining_lease_is_too_short() {
    let fixture = Fixture::new("short_lease");
    let worker = ExpiryWorker::new(
        true,
        fixture.clone(),
        Some(fixture.clone()),
        None,
        "expiry-fixture",
    )
    .unwrap();
    assert_eq!(worker.reconcile_one().await, Err(HomeBackupError::Conflict));
    assert_eq!(*fixture.calls.lock().unwrap(), vec!["claim"]);
    assert_eq!(
        fixture.backup.lock().unwrap().state,
        HomeBackupState::Expiring
    );
}

#[tokio::test]
async fn disabled_supervision_heartbeats_and_shuts_down_without_store_work() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(false, fixture.clone(), None, None, "expiry-fixture").unwrap();
    let workers = crate::worker_supervision::Workers::new();
    let (shutdown, signal) = tokio::sync::watch::channel(false);
    let task = worker.spawn_supervised(signal, workers.get("home_backup_expiry"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(fixture.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn closing_admission_recovers_expiring_without_admitting_a_ready_backup() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(
        false,
        fixture.clone(),
        Some(fixture.clone()),
        None,
        "expiry-fixture",
    )
    .unwrap();
    assert!(!worker.reconcile_one().await.unwrap());
    assert_eq!(*fixture.calls.lock().unwrap(), vec!["resume"]);
    fixture.backup.lock().unwrap().state = HomeBackupState::Expiring;
    assert!(worker.reconcile_one().await.unwrap());
    assert_eq!(
        *fixture.calls.lock().unwrap(),
        vec!["resume", "resume", "claim", "delete", "observe", "save"]
    );
    assert_eq!(
        fixture.backup.lock().unwrap().state,
        HomeBackupState::Deleted
    );
}

#[tokio::test]
async fn worker_rechecks_lease_after_each_provider_step_before_the_next_call() {
    for outcome in [
        "delete_clock_advance",
        "observe_clock_to_save_boundary",
        "observe_clock_with_save_budget",
    ] {
        let initial = layer2_core::seconds_from_rfc3339("2026-11-18T00:00:00Z").unwrap();
        let clock = Arc::new(AtomicU64::new(initial));
        let mut fixture = Fixture::new(outcome);
        Arc::get_mut(&mut fixture).unwrap().clock = Some(clock.clone());
        let worker = ExpiryWorker::new(
            true,
            fixture.clone(),
            Some(fixture.clone()),
            None,
            "expiry-fixture",
        )
        .unwrap();
        // Wall time can advance while an external future is running, independently
        // of its monotonic timeout. Recheck the lease after returning from it.
        let now = || std::time::UNIX_EPOCH + Duration::from_secs(clock.load(Ordering::SeqCst));
        let result = worker.reconcile_one_with_clock(&now).await;
        if outcome == "observe_clock_with_save_budget" {
            assert!(result.unwrap());
            assert_eq!(
                *fixture.calls.lock().unwrap(),
                vec!["claim", "delete", "observe", "save"]
            );
            assert_eq!(
                fixture.backup.lock().unwrap().state,
                HomeBackupState::Deleted
            );
        } else {
            assert_eq!(result, Err(HomeBackupError::Conflict));
            let expected = if outcome == "delete_clock_advance" {
                vec!["claim", "delete"]
            } else {
                vec!["claim", "delete", "observe"]
            };
            assert_eq!(*fixture.calls.lock().unwrap(), expected);
            assert_eq!(
                fixture.backup.lock().unwrap().state,
                HomeBackupState::Expiring
            );
            assert!(fixture.backup.lock().unwrap().erasure.is_none());
        }
    }
}

#[test]
fn call_budget_requires_strictly_more_than_its_timeout() {
    let fixture = Fixture::new("success");
    let mut value = fixture.backup.lock().unwrap().clone();
    value.lease = Some(HomeBackupLease {
        holder: format!("fixture:{}", uuid::Uuid::nil()),
        token: uuid::Uuid::nil().to_string(),
        fence: 1,
        expires_at: "2026-11-18T00:01:00Z".into(),
    });
    let expiry = std::time::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339("2026-11-18T00:01:00Z").unwrap());
    for seconds in [STORE_SECONDS, STEP_SECONDS + STORE_SECONDS] {
        let budget = Duration::from_secs(seconds);
        assert_eq!(
            ExpiryWorker::remaining(&value, budget, expiry - budget),
            Err(HomeBackupError::Conflict)
        );
        assert!(
            ExpiryWorker::remaining(&value, budget, expiry - budget - Duration::from_nanos(1))
                .is_ok()
        );
        assert_eq!(
            ExpiryWorker::remaining(&value, budget, expiry),
            Err(HomeBackupError::Conflict)
        );
    }
}

#[tokio::test]
async fn successful_delete_and_observation_with_failed_save_retain_recovery_authority() {
    let fixture = Fixture::new("save_error");
    let worker = ExpiryWorker::new(
        true,
        fixture.clone(),
        Some(fixture.clone()),
        None,
        "expiry-fixture",
    )
    .unwrap();
    assert_eq!(
        worker.reconcile_one().await,
        Err(HomeBackupError::Unavailable)
    );
    assert_eq!(
        *fixture.calls.lock().unwrap(),
        vec!["claim", "delete", "observe", "save"]
    );
    let value = fixture.backup.lock().unwrap();
    assert_eq!(value.state, HomeBackupState::Expiring);
    assert!(value.lease.is_some());
    assert!(value.erasure.is_none());
}
